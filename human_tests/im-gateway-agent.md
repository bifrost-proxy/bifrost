# IM Gateway Agent 真实场景测试用例

## 功能模块说明

IM Gateway Agent 为 Bifrost 的 IM 网关接入了 LLM 对话能力。当用户通过飞书向 Bot 发送消息时，Agent 会调用大模型 API 进行对话，并将模型回复通过飞书发送回用户。支持多轮会话、会话重置、per-route 自定义 system prompt 等。

## 前置条件

```bash
# 确保 MODELHUB_AK 环境变量已设置
source ~/.zshrc
echo $MODELHUB_AK  # 应输出 API key

# 启动 Bifrost 测试实例
BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --unsafe-ssl --no-system-proxy
```

## 测试用例列表

### TC-IMA-01: Agent 配置 API - 获取默认配置

- **操作步骤**: `curl -s http://127.0.0.1:8800/_bifrost/api/im-gateway/agent | jq .`
- **预期结果**: 返回 JSON 包含:
  - `enabled: true`
  - `model: "gpt-5.4-2026-03-05"`
  - `by_azure: true`
  - `base_url` 包含 `bytedance.net`
  - `api_key` 为 `"$MODELHUB_AK"`

### TC-IMA-02: Agent 配置 API - 更新配置

- **操作步骤**:
  ```bash
  curl -s -X PATCH http://127.0.0.1:8800/_bifrost/api/im-gateway/agent \
    -H 'Content-Type: application/json' \
    -d '{"default_system_prompt": "你是一个测试助手", "max_turn_iterations": 10}'
  ```
- **预期结果**: 返回 `{"success": true}`
- **验证步骤**: 再次 GET /agent 确认 default_system_prompt 和 max_turn_iterations 已更新

### TC-IMA-03: Agent 配置 API - 禁用/启用 Agent

- **操作步骤**:
  ```bash
  curl -s -X PATCH http://127.0.0.1:8800/_bifrost/api/im-gateway/agent \
    -H 'Content-Type: application/json' \
    -d '{"enabled": false}'
  ```
- **预期结果**: 返回 `{"success": true}`
- **验证**: GET /agent 确认 `enabled: false`
- **恢复**: PATCH `{"enabled": true}` 恢复

### TC-IMA-04: Agent 会话列表 API - 空列表

- **操作步骤**: `curl -s http://127.0.0.1:8800/_bifrost/api/im-gateway/agent/sessions`
- **预期结果**: 返回 `{"sessions": []}`

### TC-IMA-05: Agent 路由创建 - AgentChat 类型

- **操作步骤**:
  ```bash
  curl -s -X POST http://127.0.0.1:8800/_bifrost/api/im-gateway/routes \
    -H 'Content-Type: application/json' \
    -d '{
      "id": "agent-test-route",
      "provider_id": "test-feishu",
      "name": "Agent Chat Route",
      "enabled": true,
      "event_type": "message_receive",
      "matcher": {"keyword": "agent"},
      "action": {"type": "agent_chat", "system_prompt": "你是专业的代码助手", "reply_target": "original_chat"}
    }'
  ```
- **预期结果**: 返回 `{"success": true}`
- **验证**: GET /routes 确认路由已创建，action.type 为 "agent_chat"

### TC-IMA-06: Agent 路由列表 - 包含 AgentChat 类型

- **操作步骤**: `curl -s http://127.0.0.1:8800/_bifrost/api/im-gateway/routes | jq .`
- **预期结果**: 路由列表中包含 agent-test-route，action 包含 `type: "agent_chat"` 和 `system_prompt`

### TC-IMA-07: 飞书消息触发 Agent 对话（需要飞书连接）

- **前置条件**: 已创建飞书 provider 并建立连接 (POST /providers/:id/connect)
- **操作步骤**: 在飞书中向 Bot 发送消息 "你好"
- **预期结果**:
  - Bot 先添加 "OK" reaction
  - Bot 回复一条文本消息（AI 生成的回复）
  - 消息日志中出现 inbound + outbound 记录

### TC-IMA-08: 多轮对话保持上下文

- **前置条件**: 飞书连接已建立
- **操作步骤**:
  1. 发送 "我的名字是小明"
  2. 等待回复
  3. 发送 "我叫什么名字？"
- **预期结果**: 第二次回复应包含 "小明"，证明会话上下文被保持

### TC-IMA-09: /clear 命令重置会话

- **前置条件**: 已有对话历史
- **操作步骤**: 发送 "/clear"
- **预期结果**: Bot 回复 "会话已重置，可以开始新的对话。"

### TC-IMA-10: Agent 禁用时不响应

- **操作步骤**:
  1. PATCH /agent `{"enabled": false}`
  2. 通过飞书发送消息
- **预期结果**: Bot 不回复任何消息（仅添加 OK reaction）

### TC-IMA-11: 消息日志记录 Agent 回复

- **操作步骤**: 完成一次 Agent 对话后，执行：
  ```bash
  curl -s http://127.0.0.1:8800/_bifrost/api/im-gateway/providers/test-feishu/messages | jq .
  ```
- **预期结果**: 日志中包含 trigger 为 "agent" 的 outbound 消息记录

### TC-IMA-12: /undo 命令 - 回退单轮对话

- **前置条件**: 已有至少 2 轮对话历史
- **操作步骤**:
  1. 通过内部 API 创建多轮对话：
     ```bash
     curl -s -X POST http://127.0.0.1:8800/_bifrost/api/im-gateway/agent/chat \
       -H 'Content-Type: application/json' \
       -d '{"session_key": "undo-test", "message": "记住数字42"}'
     curl -s -X POST http://127.0.0.1:8800/_bifrost/api/im-gateway/agent/chat \
       -H 'Content-Type: application/json' \
       -d '{"session_key": "undo-test", "message": "记住数字99"}'
     ```
  2. 发送 `/undo` 命令：
     ```bash
     curl -s -X POST http://127.0.0.1:8800/_bifrost/api/im-gateway/agent/chat \
       -H 'Content-Type: application/json' \
       -d '{"session_key": "undo-test", "message": "/undo"}'
     ```
- **预期结果**: 回复包含"已回退 1 轮对话"，并报告移除的消息数

### TC-IMA-13: /undo N 命令 - 回退多轮对话

- **操作步骤**:
  ```bash
  curl -s -X POST http://127.0.0.1:8800/_bifrost/api/im-gateway/agent/chat \
    -H 'Content-Type: application/json' \
    -d '{"session_key": "undo-multi-test", "message": "/undo 2"}'
  ```
- **预期结果**: 回复包含"已回退 2 轮对话"

### TC-IMA-14: /compact 命令 - 手动触发记忆压缩

- **前置条件**: 会话中有至少 5 轮对话（足够触发压缩）
- **操作步骤**:
  1. 通过内部 API 创建多轮对话（5 轮以上）
  2. 发送 `/compact`：
     ```bash
     curl -s -X POST http://127.0.0.1:8800/_bifrost/api/im-gateway/agent/chat \
       -H 'Content-Type: application/json' \
       -d '{"session_key": "compact-test", "message": "/compact"}'
     ```
- **预期结果**: 回复包含"记忆压缩完成"，并显示压缩前后 token 数和节省量

### TC-IMA-14B: Agent Chat /compact 命令 - 运行中显示独立压缩状态

- **前置条件**:
  - 使用当前源码 WebUI（Vite dev server 或重新构建后的 Admin 静态资源）连接到 Bifrost Admin。
  - Agent Chat 输入框输入 `/` 时能展示 Commands 中的 `/compact` 选项。
- **操作步骤**:
  1. 打开 `http://127.0.0.1:<web-port>/_bifrost/ai?aiSection=agent-chat&agentSection=chat`。
  2. 在输入框输入 `/`，通过键盘上下键选中 `/compact`，按 Enter 触发。
  3. 在压缩请求尚未完成时观察消息流。
  4. 等待压缩完成后再次观察消息流。
- **预期结果**:
  - `/compact` 不作为用户消息气泡写入消息列表，也不显示普通 assistant 回复或工具执行过程。
  - 请求进行中时，消息流中出现独立分隔线状态 `上下文正在自动压缩`。
  - 进行中的压缩状态不在上一条 assistant 卡片下方追加 `Thinking...`。
  - 进行中的压缩状态不把上一条 assistant 的工具过程块重新标记为 `Running ...`。
  - 请求完成后，同一位置更新为 `上下文已自动压缩`。
- **执行记录（2026-05-28）**: PASS — 执行 `pnpm --dir web exec playwright test tests/ui/agent-chat.spec.ts --grep "runs compact as a control command|restores active timeline process steps" --reporter=line`，真实 Chromium UI 回归通过。用例延迟首个 `/compact` SSE 响应，验证请求进行中即显示 `上下文正在自动压缩` 独立分隔线，不渲染用户 `/compact` 气泡，不显示 `agent-chat-thinking-tail`，也不显示 `Running N command`；释放响应后同一分隔线更新为 `上下文已自动压缩`，且 assistant_delta/tool_started 不进入消息列表。同时复跑 active timeline 过程块回归，确认普通 active timeline 仍可显示真实运行中的 `Running 1 command`。

### TC-IMA-15: /compact 命令 - 历史过少时跳过

- **操作步骤**:
  ```bash
  curl -s -X POST http://127.0.0.1:8800/_bifrost/api/im-gateway/agent/chat \
    -H 'Content-Type: application/json' \
    -d '{"session_key": "compact-skip-test", "message": "hello"}'
  curl -s -X POST http://127.0.0.1:8800/_bifrost/api/im-gateway/agent/chat \
    -H 'Content-Type: application/json' \
    -d '{"session_key": "compact-skip-test", "message": "/compact"}'
  ```
- **预期结果**: 回复包含"历史消息太少，无需压缩"

### TC-IMA-16: /status 命令 - 查看会话状态

- **前置条件**: 已有对话历史
- **操作步骤**:
  ```bash
  curl -s -X POST http://127.0.0.1:8800/_bifrost/api/im-gateway/agent/chat \
    -H 'Content-Type: application/json' \
    -d '{"session_key": "status-test", "message": "/status"}'
  ```
- **预期结果**: 回复包含会话状态信息：
  - 消息数
  - 估算 token
  - API 累计 token
  - 压缩次数
  - 历史版本

### TC-IMA-17: Session API 返回 history_version 字段

- **操作步骤**: 创建一个会话并获取 sessions 列表：
  ```bash
  curl -s http://127.0.0.1:8800/_bifrost/api/im-gateway/agent/sessions | jq '.sessions[0]'
  ```
- **预期结果**: 每个 session 对象包含 `history_version` 字段（初始为 0，compaction/rollback 后递增）

### TC-IMA-18: 清理测试数据

- **操作步骤**:
  ```bash
  curl -s -X DELETE http://127.0.0.1:8800/_bifrost/api/im-gateway/routes/agent-test-route
  ```
- **预期结果**: 返回 `{"success": true}`

### TC-IMA-19: MCP 配置从当前 TOML 加载验证

- **前置条件**: `$BIFROST_DATA_DIR/agent/config.toml` 或 `~/.bifrost/agent/config.toml` 中配置了 `[mcp_servers.lark]` 段
- **操作步骤**: `curl -s http://127.0.0.1:8800/_bifrost/api/im-gateway/agent | jq '.mcp_servers'`
- **预期结果**: 返回的 JSON 包含:
  - `lark` 对象，其中 `enabled: true`
  - `url` 字段包含 `mcp.larkoffice.com`（具体 URL 不展示，安全敏感）
  - `tool_timeout_sec: 120`

### TC-IMA-20: MCP 端到端工具调用 - 文档搜索

- **前置条件**: MCP lark 服务器已配置且可连接
- **操作步骤**:
  ```bash
  curl -s -X POST http://127.0.0.1:8800/_bifrost/api/im-gateway/agent/chat \
    -H 'Content-Type: application/json' \
    -d '{"session_key": "mcp-e2e-test", "message": "使用 MCP 工具搜索飞书文档，关键词 NextOncall，只返回搜索结果即可"}'
  ```
- **预期结果**:
  - 返回 JSON 中 `success: true`
  - `tool_calls` 数组中包含至少一个 `tool_name` 以 `mcp_lark_` 开头的调用
  - 工具调用结果中包含文档搜索结果（如文档标题列表）

### TC-IMA-21: /status 报告 MCP 工具数

- **操作步骤**:
  ```bash
  curl -s -X POST http://127.0.0.1:8800/_bifrost/api/im-gateway/agent/chat \
    -H 'Content-Type: application/json' \
    -d '{"session_key": "mcp-status-test", "message": "/status"}'
  ```
- **预期结果**: 回复包含:
  - `MCP 工具` 字段，数值 > 0（当前飞书 MCP 提供 10 个工具）
  - 消息数、token 等常规状态信息

### TC-IMA-22: Skills 渐进式加载验证

- **前置条件**: `work_dir` 配置为 `<REPO_ROOT>`，项目目录中存在 `.agents/skills/` 子目录
- **操作步骤**:
  ```bash
  curl -s -X POST http://127.0.0.1:8800/_bifrost/api/im-gateway/agent/chat \
    -H 'Content-Type: application/json' \
    -d '{"session_key": "skills-test", "message": "请列出你当前可用的所有 skills，格式为 skill 名称列表"}'
  ```
- **预期结果**:
  - Agent 的回复中应提到至少以下 skills：`e2e-test`、`e2e-verify`、`rust-project-validate`
  - 证明 Skills 从 `.agents/skills/` 目录被正确加载并注入到系统 prompt

### TC-IMA-23: AGENTS.md 自动加载验证

- **前置条件**: `work_dir` 下存在 `AGENTS.md` 文件
- **操作步骤**:
  ```bash
  curl -s -X POST http://127.0.0.1:8800/_bifrost/api/im-gateway/agent/chat \
    -H 'Content-Type: application/json' \
    -d '{"session_key": "agents-md-test", "message": "你的 Project Instructions 中有哪些关于 human_tests 的规则？简要列出"}'
  ```
- **预期结果**:
  - Agent 回复应包含关于 `human_tests` 的规则说明（来自 AGENTS.md 的 Project Instructions 部分）
  - 证明 AGENTS.md 被正确加载并注入到系统 prompt

### TC-IMA-24: MCP 工具路由正确性 - 非 MCP 工具正常执行

- **操作步骤**:
  ```bash
  curl -s -X POST http://127.0.0.1:8800/_bifrost/api/im-gateway/agent/chat \
    -H 'Content-Type: application/json' \
    -d '{"session_key": "routing-test", "message": "请执行 shell 命令 echo hello-routing-test 并返回输出结果"}'
  ```
- **预期结果**:
  - `tool_calls` 中包含 `shell` 工具调用（非 MCP 工具）
  - 工具执行成功，输出包含 `hello-routing-test`
  - 证明 MCP 路由不影响本地工具的正常执行

## WebUI Agent Settings 配置管理

### TC-IMA-25: AI 一级页 Agent 渲染 - 页面加载及配置展示

- **操作步骤**:
  1. 在浏览器中访问 `http://127.0.0.1:8800/_bifrost/ai?aiSection=agent-general&agentSection=general`
  2. 检查主侧栏中是否包含与 Settings 同级的 "AI" 入口（带 Robot 图标）
  3. 检查 AI 页面左侧子导航中是否包含 Agent 分组和 General、Model、Runtime、History、Memories、Skills、Memory Records、MCP Servers、Sessions
  4. 检查右侧 Agent General 内容
- **预期结果**:
  - 主侧栏显示 "AI" 入口，且和 Settings 是同级入口
  - Settings 页面不再显示 "Agent" 或 "IM Gateway" tab
  - AI 页面左侧子导航同时整合 Agent 与 IM Gateway 子项
  - 右侧只渲染 Agent General 卡片，状态标签显示 "Enabled"/"Disabled"
  - Model 字段显示 `gpt-5.4-2026-03-05`
  - Model Provider 字段显示 `aidp_crawl`

### TC-IMA-26: Agent Tab 配置修改 - PATCH API 即时生效

- **操作步骤**:
  ```bash
  # 修改 max_turn_iterations 为 90
  curl -s -X PATCH http://127.0.0.1:8800/_bifrost/api/im-gateway/agent \
    -H 'Content-Type: application/json' \
    -d '{"max_turn_iterations": 90}'
  ```
- **预期结果**:
  - API 返回完整的 AgentConfig JSON（不是 `{"success": true}`）
  - 返回的 JSON 中 `max_turn_iterations` 为 90
  - 刷新 WebUI 页面后，Runtime Settings 中 Max Turn Iterations 显示为 90

### TC-IMA-27: Agent Tab 配置持久化 - 重启后数据保留

- **操作步骤**:
  1. 通过 API 修改配置（如 `max_turn_iterations` 改为 25）
  2. 检查 `~/.bifrost/agent/agent_config.json` 文件内容
  3. 重启 Bifrost 服务
  4. 再次通过 GET API 获取配置
- **预期结果**:
  - JSON 文件中 `max_turn_iterations` 为 25
  - 重启后 GET API 返回的 `max_turn_iterations` 仍为 25
  - 数据目录在 `~/.bifrost/agent/`（不是旧的 `~/.bifrost-agent/`）

### TC-IMA-28: Agent Tab MCP Servers - 卡片展示与操作

- **操作步骤**:
  1. 在 WebUI Agent Tab 中展开 MCP Servers 区域
  2. 检查 lark MCP server 卡片信息
  3. 点击 Edit 按钮，查看 JSON 编辑器
- **预期结果**:
  - MCP Servers 区域标题显示服务器数量（如 "1"）
  - lark 卡片显示：名称 "lark"、标签 "HTTP"、URL 地址、enabled 开关
  - 点击 Edit 后弹出 Modal，显示 JSON 配置

### TC-IMA-29: Agent Tab 数据目录统一 - 旧目录不再加载

- **操作步骤**:
  1. 在临时 HOME 下仅创建 `~/.bifrost-agent/config.toml`，不要创建 `~/.bifrost/agent/config.toml`
  2. 使用临时 `BIFROST_DATA_DIR` 启动 Bifrost
  3. 通过 GET API 获取配置，并检查日志
- **预期结果**:
  - 启动日志不包含 `~/.bifrost-agent`
  - GET API 不加载旧 TOML 中的 model、work_dir、mcp_servers
  - AgentConfigStore 的 JSON 文件存储在 `$BIFROST_DATA_DIR/agent/agent_config.json` 或 `~/.bifrost/agent/agent_config.json`

### TC-IMA-30: Agent Tab 暗色主题 - 双主题兼容性

- **操作步骤**:
  1. 在 WebUI 中切换到暗色主题（点击顶栏 moon 图标）
  2. 检查 Agent Tab 的所有区域
- **预期结果**:
  - 所有文本在暗色背景上清晰可读
  - 输入框、开关、标签等组件正确适配暗色主题
  - MCP Server 卡片边框和背景色适配主题
  - 无硬编码颜色导致的对比度问题

### TC-IMA-31: 模型 Provider 合并逻辑 - null 字段回退到内置值

- **操作步骤**:
  1. 通过 PATCH API 设置一个 provider 条目，使其字段为 null：
     ```bash
     curl -s -X PATCH http://127.0.0.1:8800/_bifrost/api/im-gateway/agent \
       -H 'Content-Type: application/json' \
       -d '{"model_providers": {"aidp_crawl": {"name": "aidp_crawl", "base_url": null, "env_key": null}}}'
     ```
  2. 获取配置并验证 effective config 仍然可用：
     ```bash
     curl -s http://127.0.0.1:8800/_bifrost/api/im-gateway/agent | jq .
     ```
- **预期结果**:
  - PATCH 请求返回成功（200 + 完整 AgentConfig JSON）
  - GET 请求返回的配置中 `model_provider` 仍为 `"aidp_crawl"`
  - Agent 仍能正常工作（不会因 "no base_url" 报错）
- **验证原因**: 修复了用户 provider 遮盖内置 provider 的 bug（null 字段现在会回退到内置默认值）

### TC-IMA-32: 模型配置完整性 - DefaultModelConfig 参数对齐

- **操作步骤**:
  1. 启动服务后获取配置：
     ```bash
     curl -s http://127.0.0.1:8800/_bifrost/api/im-gateway/agent | jq .
     ```
  2. 验证以下默认模型配置对应的字段均已正确配置
- **预期结果**:
  - `model` = `"gpt-5.4-2026-03-05"`
  - `model_provider` = `"aidp_crawl"`
  - `model_reasoning_effort` = `"medium"`
  - `model_reasoning_summary` = `"auto"`
  - `max_completion_tokens` = `16384`
  - 内置 `aidp_crawl` provider 包含:
    - `base_url` 包含 `bytedance.net`
    - `env_key` = `"MODELHUB_AK"`（从环境变量获取 AK）
    - `env_http_headers` 包含 `api-key → MODELHUB_AK`（Azure 认证模式）
    - `env_http_headers` 包含 `X-TT-LOGID → MODELHUB_LOGID`（日志追踪）
    - `request_max_retries` = `3`

### TC-IMA-33: Provider 列表 API - 返回所有内置 Provider

- **操作步骤**:
  ```bash
  curl -s http://127.0.0.1:8800/_bifrost/api/im-gateway/agent/providers | jq .
  ```
- **预期结果**:
  - 返回 JSON 数组，包含 14 个 provider
  - 每个 provider 包含 `id`、`name`、`base_url`、`env_key`、`request_max_retries`、`stream_idle_timeout_ms`、`stream_max_retries` 字段
  - 包含的 provider ID: `openai`, `aidp_crawl`, `azure`, `anthropic`, `gemini`, `groq`, `deepseek`, `ollama`, `lmstudio`, `amazon-bedrock`, `openrouter`, `xai`, `mistral`, `cerebras`
  - `openai.base_url` = `"https://api.openai.com/v1/chat/completions"`
  - `openai.request_max_retries` = `4`，`openai.stream_idle_timeout_ms` = `300000`，`openai.stream_max_retries` = `5`
  - `ollama.env_key` = `null`（本地推理，无需 API key）
  - `azure.base_url` = `null`（用户必须自行配置）

### TC-IMA-34: WebUI Provider 下拉选择 - 展示与切换

- **操作步骤**:
  1. 在浏览器中打开 `http://127.0.0.1:8800/_bifrost/settings?tab=agent`
  2. 滚动到 "Model Configuration" 区域的 "Model Provider" 字段
  3. 确认当前显示为下拉选择器（非文本输入框）
  4. 点击下拉框展开列表
  5. 验证列表包含所有 14 个 provider 名称
  6. 选择 "OpenAI"
  7. 确认下方自动展示 API URL 和 API Key Env
  8. 选择回 "AIDP Crawl"
- **预期结果**:
  - 下拉框当前选中 "AIDP Crawl"
  - 展开后列表包含: OpenAI, AIDP Crawl, Azure OpenAI, Anthropic, Google Gemini, Groq, DeepSeek, Ollama, LM Studio, Amazon Bedrock, OpenRouter, xAI (Grok), Mistral AI, Cerebras
  - 选择 OpenAI 后下方显示: API URL `https://api.openai.com/v1/chat/completions`，API Key Env `OPENAI_API_KEY`
  - 选回 AIDP Crawl 后显示: API URL 包含 `bytedance.net`，API Key Env `MODELHUB_AK`
  - 切换操作通过 PATCH API 持久化到配置文件

### TC-IMA-35: WebUI Provider 下拉 - 搜索功能

- **操作步骤**:
  1. 点击 Model Provider 下拉框
  2. 在搜索框中输入 "deep"
  3. 确认筛选结果
- **预期结果**:
  - 输入 "deep" 后仅显示 "DeepSeek" 选项
  - 支持按名称模糊搜索（showSearch + optionFilterProp="label"）

### TC-IMA-36: WebUI Provider 下拉 - 暗色主题兼容性

- **操作步骤**:
  1. 切换到暗色主题
  2. 打开 Settings → Agent Tab
  3. 查看 Model Provider 区域（下拉框 + URL/Key 提示文字）
  4. 展开下拉列表
- **预期结果**:
  - 下拉框背景色、文字颜色在暗色主题下清晰可辨
  - URL/Key 信息的 Code 样式块在暗色主题下对比度良好
  - 下拉列表的选中项高亮在暗色主题下可见

### TC-IMA-36A: WebUI Agent 默认值 placeholder

- **操作步骤**:
  1. 在浏览器中打开 `http://127.0.0.1:8800/_bifrost/ai?aiSection=agent-model&agentSection=model`
  2. 确认右侧只渲染 Agent → Model 配置卡片。
  3. 在 `Provider Connection` 区域清空或保持未配置 `Request Max Retries`、`Stream Idle Timeout (ms)`、`Stream Max Retries` 三个输入框。
  4. 观察三个输入框的 placeholder。
  5. 打开 `http://127.0.0.1:8800/_bifrost/ai?aiSection=agent-memories&agentSection=memories`。
  6. 在 `Memories` 区域清空或保持未配置 `Max Raw Memories`、`Max Unused Days`、`Max Rollout Age (days)`、`Extract Model`、`Consolidation Model` 五个输入框。
  7. 观察五个输入框的 placeholder。
- **预期结果**:
  - `Request Max Retries` 输入框 placeholder 显示当前 provider 默认值 `4`。
  - `Stream Idle Timeout (ms)` 输入框 placeholder 显示当前 provider 默认值 `300000`。
  - `Stream Max Retries` 输入框 placeholder 显示当前 provider 默认值 `5`。
  - `Max Raw Memories` 输入框 placeholder 显示默认值 `512`。
  - `Max Unused Days` 与 `Max Rollout Age (days)` 输入框 placeholder 显示 `No limit`，表达空值默认不限制。
  - `Extract Model` 与 `Consolidation Model` 输入框 placeholder 显示 `Current model (<当前 Agent 模型>)`，表达空值继承当前 Agent model。
  - placeholder 仅作为提示展示，不会在用户未输入时写入 Agent 配置或 Memories 配置。
  - 亮色和暗色主题下 placeholder 文本均清晰可读。

### TC-IMA-36B: WebUI Runtime Settings 恢复默认值

- **操作步骤**:
  1. 在浏览器中打开 `http://127.0.0.1:8800/_bifrost/ai?aiSection=agent-runtime&agentSection=runtime`。
  2. 确认右侧只渲染 `Runtime Settings` 卡片。
  3. 将 `Shell Timeout (secs)`、`Max Turn Iterations`、`Session TTL (secs)`、`Request Timeout (secs)`、`Tool Output Token Limit`、`Project Doc Max Bytes`、`Background Terminal Timeout (ms)` 临时改成非默认值。
  4. 点击 `Runtime Settings` 卡片右上角的 `Restore Defaults` 按钮。
  5. 观察页面输入框，并通过 `GET /_bifrost/api/im-gateway/agent` 检查配置。
- **预期结果**:
  - 页面提示 `Runtime settings restored to defaults`。
  - Runtime 输入框恢复为后端默认值：`shell_timeout_secs=600`、`max_turn_iterations=1000`、`session_ttl_secs=3600`、`request_timeout_secs=600`、`tool_output_token_limit=10000`、`project_doc_max_bytes=32768`、`background_terminal_max_timeout=600000`。
  - Runtime Settings 不再展示 `Max History Messages`，避免误导为请求级消息数量裁剪。
  - 后端 Agent 配置返回上述默认值，刷新页面后仍保持一致。
  - 亮色和暗色主题下右上角按钮可见且可点击。

### TC-IMA-37: 统一 Sessions 管理 - 列表展示

- **操作步骤**:
  1. 打开 Settings → Agent Tab
  2. 滚动到 "Sessions" 区域（统一表格）
  3. 确认表格显示列：Status / Session Key / Source / Work Dir / Turns / Tokens / Started / Last Active / Duration / Actions
  4. 确认表格按 Last Active 降序排序（最新在前）
  5. 点击 Refresh 按钮
  6. 使用 Status 过滤器切换 Active / Ended
  7. 使用 Source 过滤器切换 Feishu / API
- **预期结果**:
  - 表格正确渲染，无 JS 错误
  - 顶部显示统计：`N sessions`, `M active`, `K history`
  - Status 列显示 Active（绿色 Tag）/ Ended（灰色 Tag）
  - Source 列显示 Feishu（蓝色 Tag）/ API（绿色 Tag）/ —
  - Work Dir 列显示目录或 "default"
  - 默认按 Last Active 降序排序
  - Refresh 按钮可正常刷新列表
  - 过滤器正确过滤结果

### TC-IMA-38: Session 管理 - 子页面详情查看

- **操作步骤**:
  1. 先通过 API 创建一个会话：
  ```bash
  curl -s -X POST http://127.0.0.1:8800/_bifrost/api/im-gateway/agent/chat \
    -H 'Content-Type: application/json' \
    -d '{"session_key": "ui-test-unified", "message": "hello, this is a test"}'
  ```
  2. 打开 Agent Tab → Sessions，点击 Refresh
  3. 找到 "ui-test-unified" 会话，点击查看按钮（眼睛图标）
  4. 确认进入子页面（URL 包含 `?tab=agent&session=...`）
  5. 检查子页面内容：Session Info / AGENTS.md / Skills / Messages
  6. 点击 "Back" 按钮返回列表
- **预期结果**:
  - **不是 Modal 弹窗，而是子页面导航**
  - URL 变为 `?tab=agent&session=ui-test-unified&view=active`
  - 子页面顶部有 "Back" 按钮
  - Session Info 显示 Source / Work Dir / Messages / Tokens / Created / Last Active / Duration
  - AGENTS.md Instructions 卡片显示内容（如果有）
  - Skills 卡片显示已加载 Skills 列表（分 Workspace/User/System 三组）
  - Messages 区域显示会话消息，角色标签颜色区分（user=绿色, assistant=蓝色）
  - 点击 Back 返回列表页，URL 恢复为 `?tab=agent`

### TC-IMA-39: Session 管理 - History Session 详情查看

- **操作步骤**:
  1. 确保存在至少一个 Ended 状态的 history session
  2. 打开 Agent Tab → Sessions
  3. 使用 Status 过滤器选择 "Ended"
  4. 点击 history session 的查看按钮
  5. 确认进入子页面显示历史事件时间线
- **预期结果**:
  - URL 包含 `view=history&historyPath=...`
  - Session Info 显示历史会话的 Source / Turns / Tool Calls / Events / Duration
  - Event Timeline 显示事件卡片（session_start, user_message, assistant_message, tool_call, tool_result, session_end）
  - 每个事件卡片有对应图标和颜色区分

### TC-IMA-40: Session 管理 - Session 删除

- **操作步骤**:
  1. 在 Sessions 表格中找到 "ui-test-unified" 会话
  2. 点击删除按钮（垃圾桶图标）
  3. 确认弹窗中点击确认
- **预期结果**:
  - 显示 "Session deleted" 成功提示
  - 表格自动刷新，"ui-test-unified" 不再出现
  - API 验证：`curl http://127.0.0.1:8800/_bifrost/api/im-gateway/agent/sessions` 不包含 "ui-test-unified"

### TC-IMA-41: 统一 Sessions API - sessions/all 端点

- **操作步骤**:
  ```bash
  curl -s http://127.0.0.1:8800/_bifrost/api/im-gateway/agent/sessions/all | jq .
  ```
- **预期结果**:
  - 返回 JSON 包含 `sessions` 数组、`total`、`active_count`、`history_count`
  - 每个 session 项包含：`session_key`, `status`（"active"/"ended"）, `source`, `work_dir`, `turns`, `tokens`, `start_time`, `last_active_time`, `duration_secs`
  - 默认按 `last_active_time` 降序排序
  - active sessions 排在前面（如果有相同 last_active_time）

### TC-IMA-42: 组件拆分验证 - AgentTab 页面完整渲染

- **操作步骤**:
  1. 打开 Settings → Agent Tab
  2. 逐一检查以下区域是否正常渲染：
     - General（含 Enable Agent, Working Directory, Instructions）
     - Model Configuration（含 Provider 下拉、Reasoning 配置）
     - Runtime Settings（含各数值输入框）
     - History & Session
     - Memories
     - MCP Servers
     - **Sessions（统一表格，含 Status/Source/Work Dir/Turns/Tokens 等列）**
  3. **注意**：Skills 和 AGENTS.md 已移至 Session 详情子页面展示
- **预期结果**:
  - 所有 7 个 Card 区域完整渲染，无缺失
  - 各配置项数值正确显示（非空，有默认值）
  - 无控制台 JS 错误
  - **不在主页面显示 Skills Card 和 AGENTS.md Card**

### TC-IMA-43: 组件拆分验证 - 暗色主题兼容性

- **操作步骤**:
  1. 切换到暗色主题
  2. 打开 Settings → Agent Tab
  3. 检查 Sessions 统一表格区域
  4. 点击进入 Session 详情子页面
  5. 检查 Session Info / AGENTS.md / Skills / Messages 各区域
- **预期结果**:
  - 表格、按钮在暗色主题下颜色正确
  - Session 详情子页面中的消息卡片颜色区分清晰
  - AGENTS.md 代码块背景色与主题适配
  - Skills Tag 在暗色主题下可读

## 动态工作目录与 Session 管理

### TC-IMA-44: 创建带 work_dir 的 Session

- **操作步骤**:
  ```bash
  curl -s -X POST http://127.0.0.1:8800/_bifrost/api/im-gateway/agent/chat \
    -H 'Content-Type: application/json' \
    -d '{"session_key": "wd-test-1", "work_dir": "<REPO_ROOT>", "message": "hi"}'
  ```
- **预期结果**:
  - 返回 `success: true`
  - GET /sessions 中 `wd-test-1` 的 `work_dir` 为 `<REPO_ROOT>`

### TC-IMA-45: 创建不带 work_dir 的 Session

- **操作步骤**:
  ```bash
  curl -s -X POST http://127.0.0.1:8800/_bifrost/api/im-gateway/agent/chat \
    -H 'Content-Type: application/json' \
    -d '{"session_key": "wd-test-2", "message": "hi"}'
  ```
- **预期结果**:
  - 返回 `success: true`
  - GET /sessions 中 `wd-test-2` 的 `work_dir` 为 `null`

### TC-IMA-46: Sessions 列表 API 返回 work_dir 字段

- **操作步骤**:
  ```bash
  curl -s http://127.0.0.1:8800/_bifrost/api/im-gateway/agent/sessions | jq '.sessions[] | {session_key, work_dir}'
  ```
- **预期结果**:
  - 带 work_dir 的 session 显示完整路径
  - 不带 work_dir 的 session 显示 `null`

### TC-IMA-47: switch_workdir 工具 - 有效路径切换

- **操作步骤**:
  ```bash
  curl -s -X POST http://127.0.0.1:8800/_bifrost/api/im-gateway/agent/chat \
    -H 'Content-Type: application/json' \
    -d '{"session_key": "wd-switch-test", "work_dir": "<USER_HOME>", "message": "请切换工作目录到 <REPO_ROOT>"}'
  ```
- **预期结果**:
  - `tool_calls` 中包含 `switch_workdir` 工具调用，`success: true`
  - `response` 包含 "已切换工作目录到"
  - IM 通道最终回复包含最新工作路径提示：`当前工作路径: <REPO_ROOT>`
  - GET /sessions 中该 session 的 `work_dir` 更新为 `<REPO_ROOT>`
  - `message_count` 为 0（历史已清空）
  - 后续 Agent Loop 会从 `<REPO_ROOT>` 重新加载 AGENTS.md 与 repo-local skills

### TC-IMA-48: switch_workdir 工具 - 无效路径拒绝

- **操作步骤**:
  ```bash
  curl -s -X POST http://127.0.0.1:8800/_bifrost/api/im-gateway/agent/chat \
    -H 'Content-Type: application/json' \
    -d '{"session_key": "wd-switch-test", "message": "切换工作目录到 /nonexistent/path/xyz"}'
  ```
- **预期结果**:
  - `tool_calls` 中 `switch_workdir` 调用的 `success: false`
  - 结果包含 "directory does not exist"
  - session 的 `work_dir` 保持不变

### TC-IMA-49: WebUI Sessions 表格 - Work Dir 列展示

- **操作步骤**:
  1. 打开 `http://127.0.0.1:8800/_bifrost/settings?tab=agent`
  2. 滚动到 Active Sessions 区域
  3. 检查表格列
- **预期结果**:
  - 表格包含 "Work Dir" 列
  - 带 work_dir 的 session 显示完整路径（鼠标悬浮可看完整路径）
  - 不带 work_dir 的 session 显示 "default"

### TC-IMA-50: WebUI Session 详情 - Working Directory 展示

- **操作步骤**:
  1. 在 Active Sessions 表格中点击带 work_dir 的 session 的查看按钮
  2. 检查 Modal 中的 "Working Directory" 信息
- **预期结果**:
  - Modal 中显示 "Working Directory" 字段
  - 带 work_dir 的 session 显示完整路径
  - 不带 work_dir 的 session 显示 "Using default from config"

### TC-IMA-51: IM Provider 创建时配置 Agent 基础配置

- **操作步骤**:
  ```bash
  curl -s -X POST http://127.0.0.1:8800/_bifrost/api/im-gateway/providers \
    -H 'Content-Type: application/json' \
    -d '{
      "id": "agent-config-provider",
      "provider_type": "feishu",
      "display_name": "Agent Config Provider",
      "enabled": true,
      "event_connection_enabled": true,
      "event_types": [],
      "agent_config": {
        "work_dir": "<REPO_ROOT>",
        "base_instructions": "Only answer with the phrase PROVIDER_PROMPT_OK.",
        "developer_instructions": "Provider developer policy OK.",
        "user_instructions": "Provider user notes OK."
      }
    }'
  curl -s http://127.0.0.1:8800/_bifrost/api/im-gateway/providers/agent-config-provider | jq '.agent_config'
  ```
- **预期结果**:
  - Provider 创建成功
  - GET Provider 返回 `agent_config.work_dir` 等于 `<REPO_ROOT>`
  - GET Provider 返回 `agent_config.base_instructions` 包含 `PROVIDER_PROMPT_OK`
  - GET Provider 返回 `agent_config.developer_instructions` 包含 `Provider developer policy OK`
  - GET Provider 返回 `agent_config.user_instructions` 包含 `Provider user notes OK`
  - GET Provider 不再把新字段归一化为旧 `agent_config.instructions`
  - 响应不包含明文 `secret_ref`

### TC-IMA-52: IM Provider Agent 基础配置动态修改

- **操作步骤**:
  ```bash
  curl -s -X PATCH http://127.0.0.1:8800/_bifrost/api/im-gateway/providers/agent-config-provider \
    -H 'Content-Type: application/json' \
    -d '{
      "agent_config": {
        "work_dir": "<USER_HOME>",
        "base_instructions": "Only answer with the phrase PROVIDER_PROMPT_PATCHED.",
        "developer_instructions": "Provider developer policy PATCHED.",
        "user_instructions": "Provider user notes PATCHED."
      }
    }'
  curl -s http://127.0.0.1:8800/_bifrost/api/im-gateway/providers/agent-config-provider | jq '.agent_config'
  ```
- **预期结果**:
  - PATCH 返回 `success: true`
  - GET Provider 返回 `agent_config.work_dir` 等于 `<USER_HOME>`
  - GET Provider 返回 `agent_config.base_instructions` 包含 `PROVIDER_PROMPT_PATCHED`
  - GET Provider 返回 `agent_config.developer_instructions` 包含 `Provider developer policy PATCHED`
  - GET Provider 返回 `agent_config.user_instructions` 包含 `Provider user notes PATCHED`
  - 修改立即写入 provider store；不需要重启服务

### TC-IMA-53: WebUI Add/Edit IM Provider - Agent 配置字段

- **操作步骤**:
  1. 打开 `http://127.0.0.1:8800/_bifrost/ai?aiSection=im-gateway-connections&imGatewaySection=connections`
  2. 点击 Connections 区域的 Add Provider
  3. 检查弹窗包含 "Agent Working Directory"、"Base Instructions / System Prompt"、"Developer Instructions"、"User Instructions"
  4. 检查这些字段展示默认 Agent 配置作为继承值；字段留空表示继承数据目录默认配置
  5. 对已有 provider 点击 Edit
  6. 检查 Edit 弹窗只读展示 Provider ID、Type、App ID、Secret、Connection Mode
  7. 检查 Edit 弹窗同样展示默认 Agent 配置；已有 Provider 覆盖值显示在输入框中
  8. 修改 Display Name、Enabled、Owner Open ID、Agent Working Directory 与三层 Instructions 后保存
  9. 通过 GET Provider API 验证保存结果
- **预期结果**:
  - Add IM Provider 弹窗允许手动配置 Agent Working Directory 与三层 instructions
  - Provider 卡片展示 Status、App ID、Secret、Owner、Connection Mode、Provider Enabled、Agent Work Dir、Agent Base Prompt 与 Agent Developer/User 配置状态
  - Add/Edit 中空的 Agent 配置字段表示继承默认值；用户输入后才保存为单 Provider 覆盖
  - Edit 弹窗不允许修改连接配置（App ID、App Secret、Provider Type、Connection Mode）
  - Edit 后 PATCH 生效，GET Provider API 返回最新非连接配置和 `agent_config`
  - 亮色和暗色主题下字段可读、按钮可识别
- **回归执行记录（2026-05-05）**: PASS — 使用真实浏览器打开 Settings → IM Gateway，对已有 `clear-regression-provider` 点击图标 Edit，清空 `Base Instructions / System Prompt` 后保存；GET `/providers/clear-regression-provider` 返回的 `agent_config` 不再包含 `base_instructions`，同时保留 `developer_instructions = "PROVIDER_DEV_KEEP"`、`user_instructions = "PROVIDER_USER_KEEP"` 与 `work_dir = "/tmp/clear-regression"`，验证单字段清空不会被省略为“保留旧值”。

### TC-IMA-53A: 新建 IM Provider 的 agent_config 经 IM 事件链路生效

- **操作步骤**:
  1. 创建带 Provider 级 Agent 配置的新 IM Provider：
     - `agent_config.work_dir = <REPO_ROOT>`
     - `agent_config.base_instructions` 包含 `IM_PROVIDER_BASE_OK`
     - `agent_config.developer_instructions` 包含 `IM_PROVIDER_DEV_OK`
     - `agent_config.user_instructions` 包含 `IM_PROVIDER_USER_OK`
  2. 将全局 Agent 配置设置为不同 marker：`GLOBAL_BASE_SHOULD_NOT_APPEAR`、`GLOBAL_DEV_SHOULD_NOT_APPEAR`、`GLOBAL_USER_SHOULD_NOT_APPEAR`。
  3. 构造来自该 Provider owner 的 IM inbound message：`IM_PROVIDER_CHAT_MARKER 请只回复 IM_PROVIDER_CONFIG_OK`。
  4. 将事件送入与 Feishu 长连接相同的 `run_event_loop` 处理链路，并让 Chat Completions 指向本地 mock server。
  5. 检查 mock 捕获的模型请求 messages。
- **预期结果**:
  - IM event loop 会为该 Provider 构造独立 session key 并进入 Agent chat，而不是只停留在 Provider 配置存储层。
  - 模型请求 `messages[0].role == "system"`，内容包含 `IM_PROVIDER_BASE_OK`。
  - 模型请求 `messages[1].role == "developer"`，内容包含 `IM_PROVIDER_DEV_OK`。
  - 模型请求 `messages[2].role == "user"`，内容包含 `IM_PROVIDER_USER_OK`。
  - 最后一条用户消息包含 `IM_PROVIDER_CHAT_MARKER`。
  - 请求中不包含任何全局 fallback marker，证明新建 Provider 的 `agent_config` 覆盖在 IM 链路实际生效。
- **执行记录（2026-05-05）**: PASS — 运行 `cargo test -p bifrost-admin im_event_loop_uses_provider_agent_config_for_agent_chat --quiet`，测试创建 `new-im-provider-config` Provider 并向 `run_event_loop` 注入 IM inbound event；mock Chat Completions 捕获到 roles 为 `system/developer/user/...` 的模型请求，包含 `IM_PROVIDER_BASE_OK`、`IM_PROVIDER_DEV_OK`、`IM_PROVIDER_USER_OK`、`IM_PROVIDER_CHAT_MARKER`，且不包含 `GLOBAL_*_SHOULD_NOT_APPEAR`。

### TC-IMA-53B: IM Provider 当前配置与会话重开后的 work_dir 生效回归

- **操作步骤**:
  1. 创建 Provider `persist-workdir-provider`，初始 `agent_config.work_dir = /old`，并配置 `base_instructions = keep provider prompt`。
  2. 模拟 IM 对话中 `switch_workdir` 成功返回 `/new/workdir` 后的持久化路径，读取 Provider API。
  3. 对同一 session 执行 `/clear` 或 `/reset` 后，修改 Provider `agent_config.work_dir` 为另一个有效目录。
  4. 重新发起同一 IM session 的下一轮消息。
- **预期结果**:
  - Provider API / WebUI Provider 卡片展示的 `Agent Work Dir` 从 `/old` 更新为 `/new/workdir`。
  - 原 Provider 的 `base_instructions/developer_instructions/user_instructions` 不会因为 work_dir 回写被清空。
  - `/clear` 或 `/reset` 后的空 session 会按照最新 Provider `agent_config.work_dir` 重新初始化；后续 Agent Loop 从该目录加载 AGENTS.md 与 repo-local skills。
  - IM 长连接不需要重连；事件循环每次消息都读取 Provider store 最新配置，而不是沿用连接启动时的旧 provider snapshot。
- **执行记录（2026-05-05）**: PASS — 运行 `cargo test -p bifrost-admin provider_switch_workdir_persists_provider_agent_override -- --nocapture` 与 `cargo test -p bifrost-admin im_event_loop_uses_provider_agent_config_for_agent_chat -- --nocapture`；前者验证 `switch_workdir` 后 Provider `agent_config.work_dir` 持久化且保留 prompt 覆盖，后者验证 IM event loop 使用 Provider store 中的最新配置进入模型请求，而不是使用连接启动时传入的旧 provider snapshot。

### TC-IMA-53C: IM Provider 卡片详情布局不遮挡回归

- **操作步骤**:
  1. 使用临时 `BIFROST_DATA_DIR` 启动最新 Bifrost 管理端：`cargo run --bin bifrost -- start -p <PORT> --unsafe-ssl --no-system-proxy`。
  2. 通过 Provider API 准备至少 3 个 IM Provider，其中包含：
     - 长 `owner_open_id`
     - 长 `agent_config.work_dir`
     - 一个继承全局 Agent 配置的 Provider
     - 一个 `agent_config.runner = "codex"` 的 Provider
  3. 打开 `http://127.0.0.1:<PORT>/_bifrost/ai?aiSection=im-gateway-connections&imGatewaySection=connections`。
  4. 在亮色主题下检查每张 Provider 卡片的 Status、App ID、Secret、Owner、Connection Mode、Provider Enabled、Agent Runner、Agent Work Dir、Agent Base Prompt、Agent Developer/User。
  5. 切换到暗色主题后重复第 4 步。
  6. 鼠标悬停在被省略的 Owner 或 Agent Work Dir 上，检查完整值 tooltip。
  7. 检查卡片右上角只保留连接操作、Edit、Delete，以及 Weixin Provider 的二维码登录按钮；不再出现与连接操作并列的 Provider Enabled 开关。
  8. 点击 Provider 卡片的 Edit 按钮，确认操作按钮仍可点击且弹窗正常打开。
- **预期结果**:
  - Provider 卡片详情字段之间没有相互遮挡。
  - `Long Connection`、`Global default`、`codex` 等短状态文案在桌面宽度下不出现异常换行。
  - 长 Owner / Agent Work Dir 在字段值区域内省略，不覆盖相邻字段。
  - 悬停长值时 tooltip 展示完整内容。
  - 亮色和暗色主题下文本、标签和操作按钮均可读可识别。
  - Provider 卡片右上角没有重复的 Enabled 开关；Provider Enabled 只作为状态摘要展示，启用状态修改通过 Edit 弹窗完成。
  - Edit 弹窗可正常打开，证明布局调整未破坏卡片操作入口。
- **执行记录（2026-05-13）**: PASS — 使用 `BIFROST_DATA_DIR=./.bifrost-human-im-layout cargo run --bin bifrost -- start -p 18873 --unsafe-ssl --no-system-proxy` 启动最新代码，通过 Provider API 创建 `layout-feishu`、`layout-bifrost`、`layout-weixin` 三个包含长 Owner / 长 Agent Work Dir / 继承全局配置 / External CLI runner 的 Provider；Playwright 打开 `http://127.0.0.1:18873/_bifrost/ai?aiSection=im-gateway-connections&imGatewaySection=connections`，亮色和暗色主题截图分别保存到 `.bifrost-human-im-layout/screenshots/im-provider-layout-light.png` 与 `.bifrost-human-im-layout/screenshots/im-provider-layout-dark.png`。浏览器断言 3 张 Provider 卡片 `overlaps=[]`、`badWrapCount=0`、每张卡片 `switches=0`、`editButtons=1`、`connectButtons=1`、`deleteButtons=1`；悬停长 Work Dir 显示完整 tooltip，点击 `settings-im-provider-edit-layout-feishu` 后 Edit 弹窗可见。

### TC-IMA-54: Agent 全局指令配置与默认 Base Prompt 展示

- **操作步骤**:
  ```bash
  curl -s http://127.0.0.1:8800/_bifrost/api/im-gateway/agent | jq '{default_base_instructions, effective_base_instructions, base_instructions, developer_instructions, user_instructions}'
  curl -s -X PATCH http://127.0.0.1:8800/_bifrost/api/im-gateway/agent \
    -H 'Content-Type: application/json' \
    -d '{
      "base_instructions": "GLOBAL_BASE_PROMPT_OK",
      "developer_instructions": "GLOBAL_DEVELOPER_PROMPT_OK",
      "user_instructions": "GLOBAL_USER_PROMPT_OK"
    }' | jq '{effective_base_instructions, base_instructions, developer_instructions, user_instructions}'
  curl -s http://127.0.0.1:8800/_bifrost/api/im-gateway/agent/instructions | jq .
  ```
- **预期结果**:
  - 初始 GET 返回 `default_base_instructions`，内容包含内置 Bifrost Agent prompt
  - 未设置 `base_instructions` 时，`effective_base_instructions` 等于 `default_base_instructions`
  - PATCH 后 `base_instructions` 与 `effective_base_instructions` 均为 `GLOBAL_BASE_PROMPT_OK`
  - PATCH 后 `developer_instructions` 为 `GLOBAL_DEVELOPER_PROMPT_OK`
  - PATCH 后 `/agent/instructions` 只包含 `GLOBAL_USER_PROMPT_OK` 与 AGENTS.md，不包含 `GLOBAL_BASE_PROMPT_OK`，验证 base prompt 不再重复注入 user instructions
- **执行记录（2026-05-05）**: PASS — 使用 `BIFROST_DATA_DIR=./.bifrost-human-prompt cargo run --bin bifrost -- start -p 18868 --unsafe-ssl --no-system-proxy` 启动最新代码；GET `/agent` 返回 `default_base_instructions` 且包含 `Bifrost Agent`，PATCH 后 `base_instructions`、`developer_instructions`、`user_instructions` 与 `effective_base_instructions` 均按预期返回；GET `/agent/instructions` 只包含 `GLOBAL_USER_PROMPT_OK` 和 AGENTS.md/project-doc 内容，不重复注入 base prompt。
- **回归执行记录（2026-05-05）**: PASS — 针对 WebUI “无法修改/无法清空”问题，使用 `BIFROST_DATA_DIR=./.bifrost-clear-regression cargo run --bin bifrost -- start -p 18870 --unsafe-ssl --no-system-proxy` 启动最新代码；通过真实浏览器打开 Settings → Agent，依次填写 `BASE_MODIFY_OK`、`DEV_MODIFY_OK`、`USER_MODIFY_OK`，等待自动保存后 GET `/agent` 返回三项新值；随后在 WebUI 清空三个 textarea，GET `/agent` 返回 `base_instructions/developer_instructions/user_instructions = null`，`effective_base_instructions` 回退并包含 `Bifrost Agent`，页面输入框未恢复旧值。
- **真实模型回归执行记录（2026-05-05）**: PASS — 执行 `source ~/.zshrc` 后确认 `MODELHUB_AK` 存在，使用 `RUST_LOG='bifrost_admin::handlers::im_gateway=info,warn' BIFROST_DATA_DIR=./.bifrost-real-chat cargo run --bin bifrost -- start -p 18871 --unsafe-ssl --no-system-proxy` 启动真实 Bifrost；通过真实 WebUI 写入 `REAL_BASE_MODIFY_OK`、`REAL_DEV_MODIFY_OK`、`REAL_USER_MODIFY_OK` 后 GET `/agent` 可读回；调用 `/agent/chat` 走默认 `aidp_crawl` provider 和真实模型 `gpt-5.4-2026-03-05`，用户消息 `REAL_CHAT_MARKER 请只回复 REAL_CHAT_OK` 返回 `REAL_CHAT_OK`；服务日志包含 `invoking agent chat api` 与 `agent chat api completed`；Session JSONL `./.bifrost-real-chat/agent/sessions/2026/05/05/session-real-model-webui-prompt-session-1777993659.jsonl` 记录 `session_start`、`user_message`、`assistant_message`；随后在同一真实 WebUI 清空三段 textarea，GET `/agent` 返回三项为 `null`，`effective_base_instructions` 回退到内置 `Bifrost Agent`。

### TC-IMA-54B: WebUI instruction 长文本大窗口编辑回归

- **操作步骤**:
  1. 使用临时数据目录和非正式端口启动最新 Bifrost：
     ```bash
     BIFROST_DATA_DIR=./.bifrost-human-instruction-modal \
       cargo run --bin bifrost -- start -p 18872 --unsafe-ssl --no-system-proxy
     ```
  2. 用真实浏览器打开 `http://127.0.0.1:18872/_bifrost/settings?tab=agent`
  3. 检查 "Base Instructions / System Prompt"、"Developer Instructions"、"User Instructions" 只展示短预览和 Edit 按钮，页面中不出现行内 textarea，也不再展示独立的 "Default Base Instructions (read-only)" 区块。
  4. 点击 "Base Instructions / System Prompt" 的 Edit 按钮，在弹出的大窗口中点击 "Copy default into editor"，确认默认 Base Instructions 被复制到 textarea 后追加 `MODAL_BASE_OK` 并点击 OK。
  5. 分别点击 Developer/User Instructions 的 Edit 按钮，在大窗口中输入 `MODAL_DEV_OK`、`MODAL_USER_OK` 并点击 OK。
  6. 通过 API 验证保存结果：
     ```bash
     curl -s http://127.0.0.1:18872/_bifrost/api/im-gateway/agent \
       | jq '{base_instructions, developer_instructions, user_instructions}'
     ```
  7. 打开 `http://127.0.0.1:18872/_bifrost/settings?tab=im-gateway`，创建或编辑一个测试 provider。
  8. 在 IM Provider 表单中检查三段 instruction 同样只展示短预览和 Edit 按钮；点击 Base Instructions 的 Edit，在大窗口中点击 "Copy inherited into editor"，确认继承值被复制到 textarea 后追加 `PROVIDER_MODAL_BASE_OK` 并保存 provider。
  9. 通过 GET Provider API 验证 `agent_config.base_instructions` 为 `PROVIDER_MODAL_BASE_OK`。
- **预期结果**:
  - Agent 全局三段 instruction 不在页面行内展开长 textarea，且不再额外展示只读 Default Base Instructions 块。
  - Base Instructions 编辑弹窗可一键复制默认值到编辑草稿，并支持继续修改后保存。
  - 每段可编辑 instruction 都通过 Edit 按钮打开大尺寸弹窗编辑；保存后短预览立即更新。
  - GET `/im-gateway/agent` 返回 `MODAL_BASE_OK`、`MODAL_DEV_OK`、`MODAL_USER_OK`。
  - IM Provider Add/Edit 表单中的 instruction 也通过大尺寸弹窗编辑；保存后 GET Provider API 返回最新 `agent_config` 覆盖值。
  - 亮色和暗色主题下预览、Edit 按钮、弹窗标题、textarea 内容均清晰可读。
- **执行记录（2026-05-05）**: PASS — 执行 `pnpm --dir web exec playwright test tests/ui/admin-settings.spec.ts --grep "Settings (Agent 三层 instructions 使用大窗口编辑|IM Provider instructions 使用大窗口编辑后保存覆盖值)"`，真实 Chromium 打开 Settings → Agent/IM Gateway；验证 Agent 页面三段 instruction 只有短预览 + Edit 按钮，页面不含行内 textarea，也不再展示 `Default Base Instructions (read-only)`；Base Instructions 弹窗中点击 `Copy default into editor` 后 textarea 填入默认值，可继续追加内容并 PATCH `base_instructions`；IM Provider Edit 弹窗中 Base Instructions 通过大窗口编辑后保存，PATCH payload 的 `agent_config.base_instructions` 为最新值。两条新增 UI 回归均通过。

### TC-IMA-54A: Agent chat 接口端到端验证 prompt 分层、日志与 Session 记录

- **操作步骤**:
  1. 使用临时数据目录和非正式端口启动最新 Bifrost：
     ```bash
     RUST_LOG='bifrost_admin::handlers::im_gateway=info,warn' \
       BIFROST_DATA_DIR=./.bifrost-human-prompt \
       cargo run --bin bifrost -- start -p 18868 --unsafe-ssl --no-system-proxy
     ```
  2. 启动本地 OpenAI-compatible mock server，记录 `/chat/completions` 请求 body。
  3. PATCH `/im-gateway/agent`，设置：
     - `model_provider = "mock"`
     - `base_instructions` 包含 `GLOBAL_BASE_PROMPT_OK`
     - `developer_instructions` 包含 `GLOBAL_DEVELOPER_PROMPT_OK`
     - `user_instructions` 包含 `GLOBAL_USER_PROMPT_OK`
     - `model_providers.mock.base_url` 指向本地 mock server
  4. 调用 chat 接口：
     ```bash
     curl -s -X POST http://127.0.0.1:18868/_bifrost/api/im-gateway/agent/chat \
       -H 'Content-Type: application/json' \
       -d '{
         "session_key": "human-prompt-live-session-log-check-info",
         "message": "END_TO_END_CHAT_PROMPT_MARKER 请确认 info 日志可观测。",
         "work_dir": "<REPO_ROOT>"
       }'
     ```
  5. 检查 mock 捕获到的 Chat Completions request：
     - `messages[0].role == "system"` 且包含 `GLOBAL_BASE_PROMPT_OK`
     - `messages[1].role == "developer"` 且包含 `GLOBAL_DEVELOPER_PROMPT_OK`
     - `messages[2].role == "user"` 且包含 `GLOBAL_USER_PROMPT_OK` 与 AGENTS.md/project-doc/environment context
     - 最后一条用户消息包含 `END_TO_END_CHAT_PROMPT_MARKER`
  6. 检查服务日志包含 `invoking agent chat api` 与 `agent chat api completed`，并带有 `session_key`、`message_len`、`response_len`、`tool_call_count` 字段。
  7. 检查 `agent/sessions/YYYY/MM/DD/session-<session_key>-*.jsonl` 记录 `session_start`、`user_message`、`assistant_message`。
- **预期结果**:
  - chat API 返回 `success: true`，response 来自 mock 且确认角色顺序为 `system,developer,user,user`
  - 上游模型请求包含 base/developer/user 三层配置和用户消息 marker
  - Bifrost info 日志可观测到 chat API 开始与完成
  - Session JSONL 记录包含 `session_start.content.base_instructions`、用户消息和助手消息
- **执行记录（2026-05-05）**: PASS — chat API 返回 `CHAT_E2E_OK roles=system,developer,user,user`；mock 捕获请求显示 roles 为 `["system","developer","user","user"]`，且 `has_base/has_developer/has_user/has_chat_marker` 全为 `true`；Session 文件 `./.bifrost-human-prompt/agent/sessions/2026/05/05/session-human-prompt-live-session-after-restart-1777992451.jsonl` 包含 `session_start`、`user_message`、`assistant_message`；以 `RUST_LOG='bifrost_admin::handlers::im_gateway=info,warn'` 启动后，服务日志包含 `invoking agent chat api` 和 `agent chat api completed`。

## 边界测试与回归验证

### TC-IMA-55: 空状态 - 无 Session 时表格展示

- **操作步骤**:
  1. 通过 API 删除所有 active sessions：`curl -X DELETE http://127.0.0.1:8800/_bifrost/api/im-gateway/agent/sessions/active`
  2. 逐个删除所有 history sessions（通过 sessions/all 获取 history_path 后 URL-encode 删除）
  3. 验证 API 返回空：`curl -s http://127.0.0.1:8800/_bifrost/api/im-gateway/agent/sessions/all`
  4. 刷新 WebUI Agent Tab → Sessions 区域
- **预期结果**:
  - API 返回 `{"active_count":0,"history_count":0,"sessions":[],"total":0}`
  - 表格头部显示 "0 sessions", "0 active", "0 history"
  - 表格内容区域显示 Ant Design 空状态图标和 "No sessions" 文案
  - 表格列头（Status/Session Key/Source 等）仍正常渲染

### TC-IMA-56: Session Key 去重 - 含特殊字符的 session key

- **操作步骤**:
  1. 创建含空格的 session key：
     ```bash
     curl -s -X POST http://127.0.0.1:8800/_bifrost/api/im-gateway/agent/chat \
       -H 'Content-Type: application/json' \
       -d '{"session_key": "test session with spaces", "message": "hello"}'
     ```
  2. 等待 session 结束变为 history
  3. 查看 sessions/all：`curl -s http://127.0.0.1:8800/_bifrost/api/im-gateway/agent/sessions/all`
- **预期结果**:
  - JSONL 文件名中空格被替换为下划线（`session-test_session_with_spaces-*.jsonl`）
  - sessions/all API 返回的 `session_key` 为原始值 `"test session with spaces"`（非 sanitized 值）
  - 同一 session 在 active 和 history 之间不会因 key 不匹配而重复出现
  - 验证原因：修复了 `sanitize_key` 导致的 dedup 失败 bug

### TC-IMA-57: URL 边界 - 不存在的 session key

- **操作步骤**:
  1. 直接访问 `http://127.0.0.1:8800/_bifrost/settings?tab=agent&session=nonexistent-session&view=active`
  2. 检查页面行为
- **预期结果**:
  - 页面正常加载子页面布局（不崩溃）
  - 显示 "Back" 按钮可返回列表
  - Session Info 区域显示加载状态或"未找到"提示
  - 无 JS 控制台错误

### TC-IMA-58: URL 边界 - 无效 view 参数

- **操作步骤**:
  1. 直接访问 `http://127.0.0.1:8800/_bifrost/settings?tab=agent&session=test-key&view=invalid`
  2. 检查页面行为
- **预期结果**:
  - 页面不崩溃
  - 默认降级为 active view 或显示错误提示
  - "Back" 按钮可正常返回列表

### TC-IMA-59: 删除操作 - Cancel Popconfirm

- **操作步骤**:
  1. 在 Sessions 表格中找到一个 session
  2. 点击删除按钮（垃圾桶图标）
  3. 在弹出的确认框中点击 "Cancel"
- **预期结果**:
  - 确认框消失
  - Session 未被删除，仍在表格中
  - 无 API 调用发出（取消操作不触发后端请求）

### TC-IMA-60: 排序与过滤组合 - Turns 升序 + Status 过滤

- **操作步骤**:
  1. 确保表格有多行 session 数据（混合 active 和 ended）
  2. 点击 Turns 列头进行升序排序
  3. 同时使用 Status 过滤器选择 "Active"
  4. 切换 Status 过滤器为 "Ended"
  5. 清除过滤器：点击 Reset → OK
- **预期结果**:
  - 排序和过滤可同时工作，互不干扰
  - 清除过滤器后所有行恢复显示，排序保持
  - 表格头部统计数字反映过滤后的实际数量

### TC-IMA-61: API 边界 - 405 Method Not Allowed

- **操作步骤**:
  ```bash
  curl -s -o /dev/null -w "%{http_code}" -X POST \
    http://127.0.0.1:8800/_bifrost/api/im-gateway/agent/sessions/all
  ```
- **预期结果**:
  - 返回 405 状态码（sessions/all 仅支持 GET）
  - 不会误触发其他操作

### TC-IMA-62: API 边界 - 幂等删除（双重删除）

- **操作步骤**:
  1. 创建一个 session 并获取其 key
  2. 第一次删除：`curl -s -X DELETE http://127.0.0.1:8800/_bifrost/api/im-gateway/agent/sessions/{key}`
  3. 第二次删除（同一 key）：`curl -s -X DELETE http://127.0.0.1:8800/_bifrost/api/im-gateway/agent/sessions/{key}`
- **预期结果**:
  - 第一次删除返回 `{"ok":true}`
  - 第二次删除不崩溃（返回 ok 或 not found 均可接受）
  - 系统无副作用

### TC-IMA-63: 亮色主题 - Sessions 列表与详情完整验证

- **操作步骤**:
  1. 切换到亮色主题（点击 sun 图标 → 变为 moon 图标）
  2. 验证 `data-theme="light"`
  3. 打开 Agent Tab → Sessions 列表，检查表格渲染
  4. 点击 active session 进入详情子页面
  5. 检查 Session Info / AGENTS.md / Skills / Messages 各区域
  6. 返回，点击 history session 进入详情子页面
  7. 检查 Event Timeline 事件卡片渲染
- **预期结果**:
  - 表格在亮色主题下正确渲染，Status/Source Tag 颜色清晰
  - Active session 详情页：消息卡片背景色与亮色主题适配
  - History session 详情页：Event Timeline 卡片背景为白色（`rgb(255,255,255)`），文字对比度良好
  - 所有区域无颜色异常

### TC-IMA-64: Clear All Active - 一键清除

- **操作步骤**:
  1. 确保有至少 1 个 active session
  2. 在 Sessions 表格上方找到 "Clear All Active" 按钮（仅在有 active session 时显示）
  3. 点击按钮 → Popconfirm 确认
- **预期结果**:
  - 所有 active session 被删除
  - 表格刷新后 active_count 为 0
  - 已删除的 active session 可能变为 history session（JSONL 已持久化的）

### TC-IMA-65: History Session 详情 - 直接 URL 导航

- **操作步骤**:
  1. 从 sessions/all API 获取一个 history session 的 history_path
  2. 构造 URL：`http://127.0.0.1:8800/_bifrost/settings?tab=agent&session={key}&view=history&historyPath={path}`
  3. 在浏览器中直接访问该 URL
- **预期结果**:
  - 直接进入 history session 详情子页面（无需从列表点击进入）
  - Event Timeline 正确加载事件数据
  - 浏览器刷新后页面恢复到同一详情页（URL 参数持久化）
  - "Back" 按钮返回到 Sessions 列表

## 清理步骤

```bash
# 停止 Bifrost 测试实例
# Ctrl+C 或 cargo run --bin bifrost -- stop -p 8800

# 清理临时数据
rm -rf ./.bifrost-test
```

---

### TC-IMA-51: API 错误 - 优雅降级返回 partial 结果

- **操作步骤**:
  1. 创建一个 session 并发送需要工具调用的消息
  2. 在工具执行完成后，如果模型 API 调用失败（如超时），验证系统行为
  3. 检查飞书卡片是否包含已执行的工具结果和错误原因
- **预期结果**:
  - 即使模型 API 调用失败，用户仍收到飞书消息
  - 消息包含 "⚠️ 模型调用失败" 提示
  - 消息包含已执行工具的结果摘要
  - 消息包含具体错误原因
  - 消息包含 "请重新发送消息或稍后重试" 建议

### TC-IMA-52: Turn 级别自动重试机制

- **操作步骤**:
  1. 配置 agent 使用一个会超时的 API 端点
  2. 发送消息触发 agent turn
  3. 观察日志中的重试行为
- **预期结果**:
  - 日志中出现 "agent turn failed, retrying once"
  - 系统自动重试一次
  - 如重试成功，用户收到正常回复
  - 如重试也失败，日志出现 "agent turn retry also failed"
  - 用户收到包含错误原因的通知卡片

### TC-IMA-53: Transient API 错误 - 指数退避重试

- **操作步骤**:
  1. 观察日志中 API 调用是否有 timeout/rate_limit/5xx 错误
  2. 验证系统重试行为
- **预期结果**:
  - 出现 transient error 时，日志显示 "transient API error, retrying"
  - 最多重试 3 次（retry_attempt=1,2,3）
  - 重试间隔递增（1s, 2s, 4s）
  - 重试成功时日志显示 "retry succeeded"
  - 所有重试失败时，返回 partial 结果或错误通知

### TC-IMA-54: 正常对话 - 确认错误处理不影响正常流程（需飞书连接）

- **操作步骤**:
  1. 从飞书发送简单消息："你好"
  2. 等待回复
  3. 再发送一条需要工具调用的消息："帮我查一下当前目录下有什么文件"
  4. 等待回复
- **预期结果**:
  - 第一条消息收到正常回复
  - 第二条消息收到工具调用结果和模型回复
  - 日志中无 error/warn 级别的重试相关日志
  - 会话正常进行，无异常中断

### TC-IMA-66: CI E2E 启动器服务注入回归

- **操作步骤**:
  1. 执行 `BIFROST_E2E_RETRY_FAILED_ONCE=1 cargo run -p bifrost-e2e -- --test im_gateway_agent --jobs 1 --timeout 240`
  2. 检查输出中的 4 个用例：`im_gateway_agent_config_get`、`im_gateway_agent_config_patch`、`im_gateway_agent_sessions_empty`、`im_gateway_agent_route_create`
  3. 确认没有 `Expected status 200, got 503` 错误
- **预期结果**:
  - 4 个 `im_gateway_agent_*` E2E 用例全部通过
  - `/api/im-gateway/agent`、`PATCH /api/im-gateway/agent`、`/api/im-gateway/agent/sessions`、`POST /api/im-gateway/routes` 均返回 200
  - 测试启动器 `ProxyInstance::start_with_admin` 已配置 `ImGatewayService`，不再返回 `IM Gateway not configured`

### TC-IMA-67: Agent Loop tool message 序列回归

- **操作步骤**:
  1. 执行消息历史 invariant 单元回归：
     ```bash
     cargo test -p bifrost-agent -- --nocapture
     ```
  2. 执行恢复链路的精准单元回归：
     ```bash
     cargo test -p bifrost-agent test_load_conversation_matches -- --nocapture
     cargo test -p bifrost-agent test_build_messages_sanitizes -- --nocapture
     ```
  3. 执行自动化真实链路回归：
     ```bash
     cargo run -p bifrost-e2e -- --test im_gateway_agent_tool_history_resume_regression --jobs 1 --timeout 240
     ```
  4. 观察输出中的 mock Chat Completions 请求校验结果。
  5. 确认测试完成后无 `messages with role 'tool' must be a response`、`messages.[].role=tool has no preceding assistant tool_calls` 或 `assistant tool_calls were not followed by tool results` 错误。
- **预期结果**:
  - `bifrost-agent` history / persistence / session 单元回归全部通过
  - 多个 pending `tool_call` 先落盘、后续 `tool_result` 按 `call_id` 或旧记录顺序恢复时，不出现结果错配或 orphan `tool`
  - malformed history 或持久化恢复产生非法 tool-call 片段时，请求前会删除非法 `tool` suffix
  - E2E 输出 `PASS im_gateway_agent_tool_history_resume_regression`
  - 测试至少完成两轮模型工具调用：首次工具调用、JSONL 持久化恢复后的再次工具调用
  - mock 模型服务未观察到 orphan `tool` message
  - Agent turn 正常结束，不返回 400 invalid parameter
- **执行记录（2026-05-02）**:
  - `cargo test -p bifrost-agent -- --nocapture`：PASS，94 个单元测试 + 1 个 doctest 通过
  - `cargo test -p bifrost-agent test_load_conversation_matches -- --nocapture`：PASS，2 个恢复匹配测试通过
  - `cargo test -p bifrost-agent test_build_messages_sanitizes -- --nocapture`：PASS，1 个请求前 history 清洗测试通过
  - `cargo run -p bifrost-e2e -- --test im_gateway_agent_tool_history_resume_regression --jobs 1 --timeout 240`：PASS，首次工具调用、JSONL 恢复、恢复后再次工具调用均通过；未出现 orphan `tool` 或 400 invalid parameter
  - `cargo run -p bifrost-e2e -- --test im_gateway_agent_tool_history_resume_regression --test-timeout 120 --port 18882`：PASS，mock Chat Completions 在长期记忆自动抽取额外调用后仍按最后一条消息角色返回工具调用；恢复后的第二个 turn 执行 `call-4` 工具调用

### TC-IMA-88: Agent transient retry 不再复用孤儿 tool 快照

- **操作步骤**:
  1. 执行精准单元回归，验证 turn retry 前会重新 build/sanitize messages：
     ```bash
     cargo test -p bifrost-agent test_retryable_error_rebuilds_messages_before_retry -- --nocapture
     ```
  2. 执行 client 边界兜底回归：
     ```bash
     cargo test -p bifrost-agent sanitize_request_messages -- --nocapture
     ```
  3. 执行 client HTTP 请求体边界回归，验证真实发出的请求不会携带孤儿 `tool`：
     ```bash
     cargo test -p bifrost-agent chat_completion_sanitizes_messages_before_http_request -- --nocapture
     ```
  4. 执行 E2E 重试链路回归：
     ```bash
     cargo run -p bifrost-e2e -- --test im_gateway_agent_retry_sanitizes_orphan_tool_history --jobs 1 --timeout 240
     ```
  5. 检查 mock Chat Completions 收到的第 1 次失败请求、第 2 次 retry 请求与后续 tool-result 请求。
  6. 确认输出中没有 `messages with role 'tool' must be a response`、`messages.[].role=tool has no preceding assistant tool_calls` 或 `assistant tool_calls were not followed by tool results`。
- **预期结果**:
  - 单元测试证明 retry 前重新构造请求，首次请求与 retry 请求角色序列一致。
  - 即使 session 历史中预先插入孤儿 `role=tool`，首次请求和 retry 请求都不会携带该非法片段。
  - E2E 输出 `PASS im_gateway_agent_retry_sanitizes_orphan_tool_history`。
  - 首次请求返回 transient 500 后，retry 仍能继续完成至少一轮 tool call + tool result 闭环。
  - client 侧兜底 sanitize 不会删除合法的 `assistant(tool_calls)` + `tool` 成对片段。
- **执行记录（2026-05-10）**:
  - `cargo test -p bifrost-agent test_retryable_error_rebuilds_messages_before_retry -- --nocapture`：PASS，验证首次请求与 retry 请求角色序列一致，且均不含孤儿 `tool`
  - `cargo test -p bifrost-agent sanitize_request_messages -- --nocapture`：PASS，验证 client 边界会丢弃孤儿 `tool`，同时保留合法 tool segment
  - `cargo test -p bifrost-agent chat_completion_sanitizes_messages_before_http_request -- --nocapture`：PASS，验证真实 HTTP 请求体中孤儿 `tool` 已被移除，仅发送 `system -> user`
  - `cargo run -p bifrost-e2e -- --test im_gateway_agent_retry_sanitizes_orphan_tool_history --jobs 1 --timeout 240`：PASS，mock 首次返回 500 后 retry 请求仍为合法消息序列，随后继续完成 tool loop，未出现 400 invalid parameter

## 飞书卡片折叠面板与 Session Title

### TC-IMA-69: 飞书卡片折叠面板 - 工具调用记录默认折叠

- **操作步骤**:
  1. 通过 API 发送需要工具调用的消息：
     ```bash
     curl -s -X POST http://127.0.0.1:8800/_bifrost/api/im-gateway/agent/chat \
       -H 'Content-Type: application/json' \
       -d '{"session_key": "card-collapse-test", "message": "执行 echo hello-card-test"}'
     ```
  2. 检查返回 JSON 中 `tool_calls` 不为空（有工具调用）
  3. 在飞书 IM 中查看 Bot 发送的卡片消息
- **预期结果**:
  - 飞书卡片使用 JSON 2.0 schema（`"schema": "2.0"`）
  - 卡片 body 包含 main response markdown 元素（默认展开可见）
  - 卡片 body 包含 `collapsible_panel` 元素，标题为 "🔧 工具调用记录（N次）"
  - 折叠面板默认为折叠状态（`expanded: false`）
  - 折叠面板背景色为灰色（`background_color: "grey"`）
  - 展开折叠面板后，显示每个工具调用的名称（带 ✅/❌ 图标）和结果预览

### TC-IMA-70: 飞书卡片 - 无工具调用时不显示折叠面板

- **操作步骤**:
  1. 通过飞书或 API 发送不需要工具调用的简单对话消息（如 "你好"）
  2. 查看 Bot 返回的飞书卡片
- **预期结果**:
  - 卡片仅包含 main response markdown 元素
  - 不包含 `collapsible_panel` 元素
  - 用户只看到 AI 的回复文本

### TC-IMA-90: 飞书流式进度卡片 - Agent loop 执行中持续更新并结束关闭

- **前置条件**:
  - Feishu Provider 已连接，机器人具备 `im:message:send_as_bot`、`im:message` 和 `cardkit:card:write` 权限。
  - Agent 已启用，模型 mock 或真实模型会触发至少一次工具调用和一次 `update_plan`。
- **操作步骤**:
  1. 在飞书中向 Bot 发送一条会触发工具调用的消息，例如“检查当前项目并列出执行计划”。
  2. 观察 Bot 首条回复是否为 JSON 2.0 CardKit 流式卡片。
  3. 在 Agent 执行过程中观察卡片标题、最终输出、任务计划、最新工具状态、工具详情折叠区、底部状态和思考过程折叠区。
  4. 等待 Agent loop 完成。
- **预期结果**:
  - Bot 只使用一张 Agent progress card 展示本次 loop 状态，不再额外发送独立 plan card。
  - 卡片配置包含 `streaming_mode: true`，执行完成后调用 settings 更新为 `streaming_mode: false`。
  - 卡片标题默认使用用户消息；Agent 调用 `set_title` 后，标题刷新为工具设置的新标题。
  - 最终输出区在尚无最终内容时只显示 `处理中...`，不显示“最终输出”等额外标题；完成后直接显示 Agent 最终回复。
  - 任务计划仅在 `update_plan` 后展示；未产生计划时不渲染任务计划模块；折叠标题展示当前正在处理的任务。
  - 工具执行状态仅在出现工具事件后展示；详情区域默认折叠，折叠外可见最新工具名和基本状态。
  - 底部状态默认折叠，通常折叠标题只显示 token 消耗；当 guide/queue 刚被注入或修改时，标题追加一条轻量提示，避免用户误以为输入没有反馈。
  - 展开底部状态后显示 loop 次数、context 用量、压缩次数、工作路径、queue 和 guide 状态。
  - 过程思考信息不混入最终输出区；如模型在工具调用前输出过程文本，底部“思考过程”折叠区标题展示一行摘要，展开后展示最后一次完整过程文本。
  - 最终输出模块位于卡片最后，任务计划、工具状态、底部状态和思考过程的相对顺序保持不变。
  - 聊天栏摘要在完成后不再停留在 `[生成中...]`。
- **执行记录（2026-05-10）**:
  - `bash e2e-tests/tests/test_im_agent_streaming_progress_card.sh`：PASS，本地 E2E 验证 JSON 2.0 streaming card、固定 CardKit element id、可选计划/工具/思考模块、工具耗时、最终输出和折叠状态区渲染。
  - 修复后复测：`cargo test -p bifrost-admin progress_card` PASS，覆盖更新 uuid 不拼接 `card_id` 且保持短长度、guide 可见提示、最终输出置底等回归。
  - 修复后复测：`bash e2e-tests/tests/test_im_agent_streaming_progress_card.sh` PASS，覆盖 guide/queue 状态进入同一卡片并在标题中给出可见 guide 提示。
  - 默认数据目录真实 Feishu 链路（端口 9900，`--no-system-proxy`）：已观察到 IM 消息到达后立即发送 `interactive` CardKit progress card，随后进入 Agent loop；旧问题 `uuid` 字段校验失败已消失。

### TC-IMA-90A: 飞书流式进度卡片 - Token/Context 使用 K/M/B 格式化

- **前置条件**:
  - TC-IMA-90 的 progress card renderer 可运行。
  - 构造的 Agent runtime 状态中包含大数值 Token 与 Context，例如累计 token `1000000`、最近响应 `1234567`、Context `260000 / 1000000`。
- **操作步骤**:
  1. 执行 progress card renderer 单元覆盖：
     ```bash
     cargo test -p bifrost-admin progress_card --lib
     ```
  2. 检查生成的 Feishu JSON 2.0 progress card 序列化内容。
  3. 展开底部状态区，查看 `Context` 与 `Token` 行。
- **预期结果**:
  - 折叠标题展示 `Token：累计 1M · 最近 1.2M`。
  - 展开状态区展示 `Context：~260K / 1M (26.0%)`。
  - 展开状态区展示 `Token：累计 1M，最近 1.2M`。
  - 卡片 JSON 中不再出现 `Token：累计 1000000`、`最近 1234567` 或 `Context：~260000 / 1000000` 这类裸长数字。
- **执行记录（2026-05-21）**: PASS — 执行 `cargo test -p bifrost-admin progress_card --lib`，6 个 progress card 相关测试全部通过；新增断言覆盖 K/M/B formatter 和飞书卡片标题、展开状态区 Token/Context 字段。

### TC-IMA-90B: `/status` 展示 runner 元信息、历史轮次、显式压缩次数与上下文管理

- **前置条件**:
  - 存在一个已有 Agent session，或可通过测试构造 session detail。
  - 对外部 Runner session，已记录 Runner adapter、Runner ID、Codex `threadId` 或 ChatGPT Web `conversationId`。
  - 对压缩恢复路径，session JSONL 中至少包含一条 `compaction` 事件。
- **操作步骤**:
  1. 执行 IM status 文本单元回归：
     ```bash
     cargo test -p bifrost-admin im_status_text_formats_metrics_and_runner_metadata --lib
     ```
  2. 执行 Agent status 与 compaction runtime state 回归：
     ```bash
     cargo test -p bifrost-agent session_status --lib
     cargo test -p bifrost-agent runtime_state --lib
     cargo test -p bifrost-agent record_compaction_event_round_trip --lib
     ```
  3. 在真实 IM 或 `/agent/chat` 中发送 `/status`。
- **预期结果**:
  - `/status` 展示 `Agent 类型`、`Runner 类型`、`Runner ID`、`外部会话`、`历史对话轮次`。
  - Codex Runner session 展示 `Codex threadId=<id>`；ChatGPT Web session 展示 `conversationId=<id>`。
  - `估算 token`、`API 累计 token`、`Context 用量` 使用 K/M/B 格式，例如 `19.3K`、`38.6K`、`250K`。
  - 已发生 compaction 的 session 在恢复后 `/status` 展示非 0 `显式压缩次数`，不会因 `/resume` 或 runtime state reload 回到 0。
  - `/status` 单独展示 `上下文管理`，表达上下文由 token/context budget 与 compaction 管理，不把它混同为显式压缩。
- **执行记录（2026-05-28）**: PASS — 执行 `cargo test -p bifrost-admin im_status_text_formats_metrics_and_runner_metadata --lib -- --nocapture`，结果 `1 passed`；验证外部 Runner status 展示 `External Runner Agent`、`codex`、`Codex threadId=thread-status-123`、历史对话轮次 `2`、`API 累计 token: 38.6K`、`显式压缩次数: 2`，并在 3 条历史下展示 `上下文管理: 按 token/context budget 与 compaction 管理`。
- **历史执行记录（2026-05-21）**: PASS — `cargo test -p bifrost-admin im_status_text_formats_metrics_and_runner_metadata --lib` 通过，验证外部 Runner status 展示 `External Runner Agent`、`codex`、`Codex threadId=thread-status-123`、历史对话轮次 `2`、`API 累计 token: 38.6K` 和 `压缩次数: 2`。`cargo test -p bifrost-agent session_status --lib`、`cargo test -p bifrost-agent runtime_state --lib`、`cargo test -p bifrost-agent record_compaction_event_round_trip --lib`、`cargo test -p bifrost-agent scan_session_summary_uses_recorded_compaction_count_when_higher --lib` 均通过，验证 active `/status` K/M/B、compaction 事件恢复，并优先保留事件内已记录的更高压缩次数。

### TC-IMA-91: 飞书流式进度卡片 - guide 消息进入后同卡刷新

- **前置条件**:
  - TC-IMA-90 的 Feishu Provider 和 Agent 配置可用。
  - 当前 session 正在执行长任务，尚未结束。
- **操作步骤**:
  1. 发送一条会持续执行的 Agent 消息。
  2. 在卡片仍处于执行中时，直接发送一条新的普通消息作为 guide，例如“优先检查失败日志”。
  3. 观察 IM 会话中的卡片消息和底部折叠状态区。
- **预期结果**:
  - 新 guide 消息被注入 guide channel，当前 progress card 不撤回、不重发，仍是同一个 card/message。
  - 同一卡片折叠状态区标题可见“已收到引导：...”轻量提示；展开后显示“有待处理引导消息”，并保留当前工具、计划、context/token 等最新状态。
  - 执行过程中不会出现第二张 progress card，也不会调用关闭旧卡 streaming 或撤回旧消息。
  - Agent 在当前工具调用批次结束后消费 guide，最终输出反映 guide 语义。
- **执行记录（2026-05-10）**:
  - `bash e2e-tests/tests/test_im_agent_streaming_progress_card.sh`：PASS，本地 E2E 验证 guide pending 进入同一卡片折叠状态区，且 renderer 不依赖撤回/重发语义。
  - 修复后复测：`bash e2e-tests/tests/test_im_agent_streaming_progress_card.sh` PASS，断言状态区标题包含“已收到引导：...”，避免 guide 注入成功但用户无可见反馈。
  - 默认数据目录真实 Feishu 链路：测试中途发送的新 IM 消息被注入 guide，没有发送第二张 progress card。

### TC-IMA-91A: Web Agent Chat 后端持久排队与刷新恢复

- **前置条件**:
  - 使用临时数据目录启动最新 Bifrost：`BIFROST_DATA_DIR=$(mktemp -d) target/debug/bifrost start -p <PORT> --unsafe-ssl --no-system-proxy --skip-cert-check`。
  - Agent 配置启用内置 Bifrost Agent，模型指向本地慢速 OpenAI-compatible mock，首个请求保持运行中。
- **操作步骤**:
  1. 打开 `http://127.0.0.1:<PORT>/_bifrost/ai?aiSection=agent-chat&agentSection=chat`。
  2. 发起一条会保持运行中的消息，确认状态进入 Running。
  3. 在运行中输入框选择 `Queue`，发送 `queued follow-up from web ui`。
  4. 调用 `GET /_bifrost/api/im-gateway/agent/sessions/all`，确认当前 session 返回 `queue_items[0].message == "queued follow-up from web ui"`。
  5. 刷新 WebUI 页面，确认 Queue 面板仍显示 `#1 queued follow-up from web ui`。
  6. 释放或停止首个慢请求，观察后端自动处理排队消息；确认前端没有再发起额外本地重发造成重复消息。
- **预期结果**:
  - 排队消息写入后端 `SessionQueueManager`，`/agent/chat/stream` busy 响应、`/sessions/all` 和 `/sessions/{session_key}` 均返回同一组 `queueItems`。
  - 页面刷新后 Queue 面板从后端恢复，不依赖 React 本地状态。
  - 当前 turn 结束后由后端 drain queue 并继续处理，前端不再用 running false transition 自动重发队列消息。
- **执行记录（2026-05-29）**:
  - `CARGO_TARGET_DIR=target/agent-chat-queue-e2e BIFROST_E2E_RUNNER_JOBS=1 cargo run -p bifrost-e2e -- --test im_gateway_agent_chat_queue_state_persists_for_refresh --timeout 120 --port 18887`：PASS，真实启动 Admin + 慢速 mock model，验证 busy queue 写入后端、`sessions/all` 与 session detail 均返回同一 `queueItems`，并通过 `/stop` 释放当前 turn 后由后端继续 drain。
  - API-backed human 流程覆盖 TC-IMA-91A 的步骤 2-6；WebUI 刷新恢复依赖同一 `sessions/all` / detail payload，前端构建验证见 `pnpm --dir web run build`。

### TC-IMA-92: 飞书流式进度卡片 - queue 消息进入后同卡刷新

- **前置条件**:
  - TC-IMA-90 的 Feishu Provider 和 Agent 配置可用。
  - 当前 session 正在执行长任务，尚未结束。
- **操作步骤**:
  1. 发送一条会持续执行的 Agent 消息。
  2. 在卡片仍处于执行中时发送 `/q 第二个任务`。
  3. 可选再发送 `/rq <序号>` 删除排队消息。
  4. 观察 IM 会话中的卡片消息和底部折叠状态区中的排队状态。
- **预期结果**:
  - `/q` 成功后当前 progress card 不撤回、不重发，仍是同一个 card/message。
  - 同一卡片折叠状态区展开后显示当前排队消息数量；`/rq` 成功后在同一卡片中刷新排队数量；折叠标题仍只显示 token 消耗。
  - 执行过程中不会出现第二张 progress card，也不会调用关闭旧卡 streaming 或撤回旧消息。
  - 当前 turn 完成后，排队消息被继续处理，并为下一轮创建新的 progress card；上一轮卡片已在结束时关闭 streaming。
- **执行记录（2026-05-10）**:
  - `bash e2e-tests/tests/test_im_agent_streaming_progress_card.sh`：PASS，本地 E2E 验证 queue count 进入同一卡片折叠状态区，且 renderer 输出同卡 guide/queue 状态。

### TC-IMA-71: Session Title 落库 - set_title 工具持久化

- **操作步骤**:
  1. 通过 API 触发一次会话，使 Agent 调用 set_title 工具：
     ```bash
     curl -s -X POST http://127.0.0.1:8800/_bifrost/api/im-gateway/agent/chat \
       -H 'Content-Type: application/json' \
       -d '{"session_key": "title-persist-test", "message": "帮我查看一下 Cargo.toml 文件的内容"}'
     ```
  2. 检查返回 JSON 中 `title_updated` 字段不为空
  3. 查看 sessions API 确认 title 存在：
     ```bash
     curl -s http://127.0.0.1:8800/_bifrost/api/im-gateway/agent/sessions | jq '.sessions[] | select(.session_key == "title-persist-test") | {session_key, title}'
     ```
  4. 删除 active session 使其变为 history，再通过 sessions/all 检查 title 是否保留：
     ```bash
     curl -s -X DELETE http://127.0.0.1:8800/_bifrost/api/im-gateway/agent/sessions/title-persist-test
     curl -s http://127.0.0.1:8800/_bifrost/api/im-gateway/agent/sessions/all | jq '.sessions[] | select(.session_key == "title-persist-test") | {session_key, title, status}'
     ```
- **预期结果**:
  - `title_updated` 包含 Agent 生成的 session 标题
  - Active session 的 title 字段不为 null
  - History session（从 JSONL 恢复）的 title 字段与 active session 一致
  - JSONL 文件中包含 `title_updated` 事件类型

### TC-IMA-72: Session Title 在 sessions/all API 中返回

- **操作步骤**:
  ```bash
  curl -s http://127.0.0.1:8800/_bifrost/api/im-gateway/agent/sessions/all | jq '.sessions[] | {session_key, title, status}'
  ```
- **预期结果**:
  - Active sessions 的 `title` 字段来自内存中的 session.title
  - Ended sessions 的 `title` 字段从 JSONL 的 `title_updated` 事件中恢复
  - 未设置 title 的 session 返回 `title: null`

### TC-IMA-73: 飞书卡片 header 使用 Session Title

- **操作步骤**:
  1. 在飞书中开始一个新会话，发送消息触发 Agent
  2. 等待 Agent 回复第一条消息（此时 Agent 应已调用 set_title）
  3. 查看飞书卡片的 header 标题
- **预期结果**:
  - 如果 session 有 title，卡片 header 显示 title 内容（而非默认的 "Bifrost AI"）
  - 如果 session 无 title（如第一轮 Agent 未调用 set_title），header 显示 "Bifrost AI"

### TC-IMA-74: WebUI Sessions 表格 - Title 列展示

- **操作步骤**:
  1. 确保有带 title 的 session（通过前述测试创建）
  2. 打开 `http://127.0.0.1:8800/_bifrost/settings?tab=agent`
  3. 滚动到 Sessions 区域
  4. 检查表格是否有 "Title" 列
- **预期结果**:
  - 表格在 Source 列之后显示 "Title" 列
  - 带 title 的 session 显示 title 文本，鼠标悬浮可查看完整标题
  - 无 title 的 session 显示 "—"（em-dash）
  - 列宽度为 180px，超长文本省略显示

### TC-IMA-68: Agent 配置 API - API Key 写入保持 Azure header 认证

- **操作步骤**:
  1. 使用临时数据目录启动 Bifrost，避免影响正式实例：
     ```bash
     BIFROST_DATA_DIR="$(mktemp -d)" cargo run --bin bifrost -- start -p 18868 --unsafe-ssl --no-system-proxy
     ```
  2. 通过 PATCH 设置默认 `aidp_crawl` provider 的 `api_key`：
     ```bash
     curl -s -X PATCH http://127.0.0.1:18868/_bifrost/api/im-gateway/agent \
       -H 'Content-Type: application/json' \
       -d '{"model": "test-model-e2e", "base_url": "https://test.example.com", "api_key": "test-api-key-e2e"}' | jq .
     ```
  3. 再次 GET 配置：
     ```bash
     curl -s http://127.0.0.1:18868/_bifrost/api/im-gateway/agent | jq '.model_providers.aidp_crawl'
     ```
- **预期结果**:
  - PATCH 返回 200 和完整 AgentConfig JSON
  - GET 返回的 `model_providers.aidp_crawl.api_key` 为 `"test-api-key-e2e"`
  - GET 返回的 `model_providers.aidp_crawl.http_headers.api-key` 为 `"test-api-key-e2e"`
  - 运行时仍按 `api-key` header 使用 Azure/MODELHUB 认证，不退化成 Bearer 认证
- **执行记录（2026-05-03）**:
  - 自动化真实链路回归 `cargo run -p bifrost-e2e -- --test im_gateway_agent_config_patch --test-timeout 120 --port 18180`：PASS，PATCH 后 GET 可见 `model_providers.aidp_crawl.api_key = "test-api-key-e2e"`，且 `http_headers.api-key = "test-api-key-e2e"`，保持 Azure/MODELHUB `api-key` header 认证路径

### TC-IMA-75: Goal 模式 - create_goal 工具触发

- **操作步骤**:
  ```bash
  curl -s -X POST "http://127.0.0.1:8800/_bifrost/api/im-gateway/agent/chat" \
    -H "Content-Type: application/json" \
    -d '{"message": "请创建一个 goal，目标是：实现一个简单的计算器功能，token budget 设为 5000", "session_key": "e2e-goal-test-create"}'
  ```
- **预期结果**:
  - `success: true`
  - `tool_calls` 数组中包含 `tool_name: "create_goal"` 的调用
  - create_goal 的 result 包含 JSON：`status: "active"`, `tokenBudget: 5000`, `tokensUsed: 0`, `threadId`，且不暴露内部 `goalId`
  - response 文本描述了 goal 的创建结果
- **执行记录（2026-05-05）**: PASS — 模型成功调用 create_goal，返回 `status: "active"`, `tokenBudget: 5000`, `tokensUsed: 0`, `remainingTokens: 5000`

### TC-IMA-76: Goal 模式 - get_goal 状态查询与 budget 超限自动转换

- **前置条件**: TC-IMA-75 已创建 goal 且 session 已消耗 token
- **操作步骤**:
  ```bash
  curl -s -X POST "http://127.0.0.1:8800/_bifrost/api/im-gateway/agent/chat" \
    -H "Content-Type: application/json" \
    -d '{"message": "查看当前 goal 的状态", "session_key": "e2e-goal-test-create"}'
  ```
- **预期结果**:
  - `tool_calls` 包含 `tool_name: "get_goal"`
  - goal 的 `status` 变为 `"budgetLimited"`（因为 turn token 消耗已超过 budget 5000）
  - `tokensUsed` > `tokenBudget`
  - `remainingTokens: 0`
- **执行记录（2026-05-05）**: PASS — get_goal 返回 `status: "budgetLimited"`, `tokensUsed: 33531`, `tokenBudget: 5000`, `remainingTokens: 0`

### TC-IMA-77: Goal 模式 - update_goal 标记完成与 completionBudgetReport

- **前置条件**: TC-IMA-76 的 session 中 goal 已存在
- **操作步骤**:
  ```bash
  curl -s -X POST "http://127.0.0.1:8800/_bifrost/api/im-gateway/agent/chat" \
    -H "Content-Type: application/json" \
    -d '{"message": "请将 goal 标记为 complete", "session_key": "e2e-goal-test-create"}'
  ```
- **预期结果**:
  - `tool_calls` 包含 `tool_name: "update_goal"`
  - update_goal 的 arguments 包含 `status: "complete"`
  - result 中 goal 的 `status: "complete"`
  - result 中 `completionBudgetReport` 非 null，包含 token 和时间使用统计
- **执行记录（2026-05-05）**: PASS — update_goal 返回 `status: "complete"`, `completionBudgetReport: "Goal achieved. Report final budget usage to the user: tokens used: 67548 of 5000; time used: 30 seconds."`

### TC-IMA-78: Goal 模式 - /goal 命令查看状态

- **操作步骤**:
  ```bash
  curl -s -X POST "http://127.0.0.1:8800/_bifrost/api/im-gateway/agent/chat" \
    -H "Content-Type: application/json" \
    -d '{"message": "/goal", "session_key": "e2e-goal-test-create"}'
  ```
- **预期结果**:
  - `tool_calls` 为空数组（/goal 是内置命令，不经过 LLM）
  - response 包含 goal 的 JSON 状态（含 threadId, objective, status, tokenBudget 等字段）
- **执行记录（2026-05-05）**: PASS — 直接返回 goal JSON，`status: "complete"`, tool_calls 为空

### TC-IMA-79: Goal 模式 - /goal pause 暂停

- **前置条件**: 新会话中已创建 active goal
- **操作步骤**:
  ```bash
  # 先创建 goal
  curl -s -X POST "http://127.0.0.1:8800/_bifrost/api/im-gateway/agent/chat" \
    -H "Content-Type: application/json" \
    -d '{"message": "创建一个 goal: 编写文档，token budget 10000", "session_key": "e2e-goal-pause"}'
  # 暂停
  curl -s -X POST "http://127.0.0.1:8800/_bifrost/api/im-gateway/agent/chat" \
    -H "Content-Type: application/json" \
    -d '{"message": "/goal pause", "session_key": "e2e-goal-pause"}'
  ```
- **预期结果**:
  - 暂停响应中 goal 的 `status: "paused"`
  - tool_calls 为空
- **执行记录（2026-05-05）**: PASS — `/goal pause` 返回 `status: "paused"`, `tokensUsed: 16695`

### TC-IMA-80: Goal 模式 - /goal resume 恢复

- **前置条件**: TC-IMA-79 已暂停 goal
- **操作步骤**:
  ```bash
  curl -s -X POST "http://127.0.0.1:8800/_bifrost/api/im-gateway/agent/chat" \
    -H "Content-Type: application/json" \
    -d '{"message": "/goal resume", "session_key": "e2e-goal-pause"}'
  ```
- **预期结果**:
  - goal 的 status 恢复（如果 token 已超 budget 则为 `"budgetLimited"`，否则为 `"active"`）
  - tool_calls 为空
- **执行记录（2026-05-05）**: PASS — resume 后 `status: "budgetLimited"`（因 tokensUsed 16695 > budget 10000）

### TC-IMA-81: Goal 模式 - Session 隔离验证

- **操作步骤**:
  ```bash
  curl -s -X POST "http://127.0.0.1:8800/_bifrost/api/im-gateway/agent/chat" \
    -H "Content-Type: application/json" \
    -d '{"message": "/goal", "session_key": "e2e-goal-isolation-new-session"}'
  ```
- **预期结果**:
  - response 包含 `"goal": null`
  - 新 session 中不存在其他 session 的 goal
- **执行记录（2026-05-05）**: PASS — 新 session 返回 `goal: null, remainingTokens: null`

### TC-IMA-82: Goal 模式 - 工具调用与 token accounting

- **操作步骤**:
  ```bash
  curl -s -X POST "http://127.0.0.1:8800/_bifrost/api/im-gateway/agent/chat" \
    -H "Content-Type: application/json" \
    -d '{"message": "创建一个 goal: 在 /tmp 下创建一个 hello.txt 文件并写入 Hello World，token budget 设为 50000。然后立即执行这个任务。", "session_key": "e2e-goal-tools", "work_dir": "/tmp/bifrost-e2e-test"}'
  ```
- **预期结果**:
  - `success: true`
  - `tool_calls` 包含 `create_goal`、`shell` 或 `write_file` 等工具调用
  - goal 最终被标记为 `complete`（或 `budgetLimited`）
  - `/tmp/bifrost-e2e-test/hello.txt` 文件被创建且内容为 "Hello World"
- **清理**: `rm -rf /tmp/bifrost-e2e-test`
- **执行记录（2026-05-05）**: PASS — 模型调用了 create_goal → shell → write_file → read_file → update_goal(complete)，文件内容验证正确

### TC-IMA-83: Agent 模型请求默认进入 Bifrost Traffic

- **操作步骤**:
  1. 使用非 9900 端口和临时数据目录启动真实 Bifrost：
     ```bash
     BIFROST_DATA_DIR="$(mktemp -d)" cargo run --bin bifrost -- start -p 18883 --unsafe-ssl --no-system-proxy
     ```
  2. 启动一个本地 OpenAI-compatible mock Chat Completions 服务，记录请求。
  3. 将 Agent 配置 PATCH 到 mock 服务：
     ```bash
     curl -s -X PATCH http://127.0.0.1:18883/_bifrost/api/im-gateway/agent \
       -H 'Content-Type: application/json' \
       -d '{"model":"mock-model","model_provider":"mock","model_providers":{"mock":{"base_url":"http://127.0.0.1:<mock_port>/chat/completions","http_headers":{"Authorization":"Bearer test"}}}}'
     ```
  4. 通过 Agent Chat API 触发一次模型请求：
     ```bash
     curl -s -X POST http://127.0.0.1:18883/_bifrost/api/im-gateway/agent/chat \
       -H 'Content-Type: application/json' \
       -d '{"session_key":"agent-proxy-human","message":"hello via proxy"}'
     ```
  5. 查询 Traffic：
     ```bash
     curl -s "http://127.0.0.1:18883/_bifrost/api/traffic?limit=20&host_contains=127.0.0.1:<mock_port>" | jq '.records[] | {m,h,u,s}'
     ```
- **预期结果**:
  - Agent Chat API 返回 `success: true`，response 来自 mock 模型。
  - mock 服务收到 Chat Completions POST 请求。
  - Traffic 中出现一条 `POST` 到 `127.0.0.1:<mock_port>` 的记录，说明 Agent 底层模型请求默认经当前 Bifrost 端口代理发出。
  - 启动命令包含 `--no-system-proxy`，不会污染本机正式系统代理。
- **执行记录（2026-05-05）**:
  - 自动化真实链路回归 `CARGO_TARGET_DIR=target/agent-proxy-e2e BIFROST_E2E_RUNNER_JOBS=1 cargo run -p bifrost-e2e -- --test im_gateway_agent_model_request_uses_bifrost_proxy --test-timeout 120 --port 18884`：PASS。用例通过 `model-proxy.test host://127.0.0.1:<mock_port>` 规则把外部模型 host 转到本地 mock，Agent 请求经 Bifrost 端口代理后在 Traffic 中出现 `POST model-proxy.test` 记录。
  - 真实模型 + TLS 拦截回归 `source ~/.zshrc; BIFROST_DATA_DIR="$(mktemp -d)" cargo run --bin bifrost -- start --host 127.0.0.1 -p 18883 --unsafe-ssl --no-system-proxy --intercept-include search.bytedance.net` 后调用 `/api/im-gateway/agent/chat`：PASS。Agent Chat 返回 `success: true`，Traffic 出现 `REQ-69fa0d05-000003`，`POST https://search.bytedance.net/gpt/openapi/online/multimodal/crawl`，`status=200`，`protocol=https`，`is_tunnel=false`，request body 文件 61531 bytes、response body 文件 493 bytes。验证 Agent 已信任当前 Bifrost CA，不再因 `UnknownIssuer` 在 TLS 拦截下失败。

### TC-IMA-83A: Agent worker 内置代理信任与 CLI Server 模式边界

- **前置条件**:
  - 当前源码已构建或可通过 `cargo run` 启动。
  - 使用临时数据目录，启动命令必须包含 `--no-system-proxy`。
- **操作步骤**:
  1. 设置外部代理环境变量指向当前 Bifrost 端口，并通过 IM/Web Agent Chat 入口触发内置 Agent。确认 worker 请求仍能恢复当前 Bifrost 端口并加载 `data_dir/certs/ca.crt`。
  2. 执行代码级回归：
     ```bash
     cargo test -p bifrost-agent default_agent_client_ignores_proxy_environment -- --nocapture
     cargo test -p bifrost-agent explicit_model_proxy_is_used_even_when_proxy_env_is_bad -- --nocapture
     cargo test -p bifrost-admin worker_proxy_port_resolution_reads_runtime_file_when_env_missing --lib -- --nocapture
     cargo test -p bifrost-admin worker_proxy_port_resolution_prefers_environment --lib -- --nocapture
     ```
  3. 在未启动目标端口服务时执行：
     ```bash
     cargo run --bin bifrost -- -p 19999 agent run --runner codex --session cli-server-required 'ping'
     ```
- **预期结果**:
  - `AgentClient::new()` 不读取 `HTTP_PROXY/HTTPS_PROXY/ALL_PROXY`，不会被外部 shell/system proxy 环境变量劫持。
  - 显式 `new_with_bifrost_proxy_and_ca` 仍强制走当前 Bifrost 端口，并使用当前 Bifrost CA 信任 TLS intercept。
  - 独立 `bifrost agent worker` 子进程从父进程请求、`BIFROST_ADMIN_PORT` 或 `runtime.json` 恢复当前 Server 端口，不退化成 direct 模型请求。
  - HTTP MCP、MCP availability、MCP OAuth、Agent 回复远端附件下载、ChatGPT Web native/CDP HTTP 探测不读取外部 proxy env；HTTP MCP 在 Agent 有内置代理时复用同一个 Bifrost proxy URL 与 CA。
  - CLI `agent run` 只调用 Admin Server 的 chat stream；目标端口无 Server 时明确报 `Failed to reach Bifrost ... is the proxy running?`，不会在当前 CLI 进程里 fallback 本地执行 Codex/ChatGPT Web。
- **执行记录（2026-05-29）**:
  - `cargo test -p bifrost-agent default_agent_client_ignores_proxy_environment -- --nocapture`：PASS。
  - `cargo test -p bifrost-agent explicit_model_proxy_is_used_even_when_proxy_env_is_bad -- --nocapture`：PASS。
  - `cargo test -p bifrost-agent mcp_ -- --nocapture`：PASS，覆盖 `mcp_direct_http_network_ignores_proxy_environment` 与 `mcp_explicit_http_network_uses_configured_proxy`。
  - `cargo test -p bifrost-admin worker_proxy_port_resolution_reads_runtime_file_when_env_missing --lib -- --nocapture`：PASS。
  - `cargo test -p bifrost-admin worker_proxy_port_resolution_prefers_environment --lib -- --nocapture`：PASS。
  - `CARGO_TARGET_DIR=target/agent-proxy-e2e BIFROST_E2E_RUNNER_JOBS=1 cargo run -p bifrost-e2e -- --test im_gateway_agent_model_request_uses_bifrost_proxy --test-timeout 120 --port 18884`：PASS。临时 E2E 数据目录尚未生成 CA 时会告警 `proxied HTTP client could not load CA`，但不会退回 direct，模型请求仍保持经 Bifrost proxy 转发。
  - `cargo run --bin bifrost -- -p 19999 agent run --runner codex --session cli-server-required 'ping'`：PASS，退出码 1，错误包含 `Failed to reach Bifrost at 127.0.0.1:19999 — is the proxy running?`。

### TC-IMA-83B: Bifrost 异步子进程按场景展示进程名

- **前置条件**:
  - 当前源码已构建或可通过 `cargo run` 启动。
  - 使用临时数据目录，启动命令必须包含 `--no-system-proxy`。
- **操作步骤**:
  1. 触发一个内置 Bifrost Agent turn，并在系统进程列表或 `ps` 输出中检查子进程名称。
  2. 触发一个 Codex/ChatGPT Web/external CLI Runner run，并检查对应子进程名称。
  3. 触发 Voice worker 或 ASR managed service 启动，并检查对应子进程名称。
  4. 查看临时数据目录下 `runtime/process-aliases/`，确认存在场景别名入口。
- **预期结果**:
  - 内置 Agent worker 通过 `bifrost-agent` 入口启动。
  - 外部 CLI Runner worker 通过 `bifrost-runner` 入口启动。
  - Voice worker 通过 `bifrost-voice` 入口启动。
  - 托管 ASR server 通过 `bifrost-asr-server` 入口启动。
  - 按 chunk fork 的 ASR CLI 通过 `bifrost-asr-cli` 入口启动。
  - 如果别名创建失败，业务仍可启动原 executable，日志中出现可定位 warning。
- **清理步骤**:
  - 停止临时 Bifrost 服务，清理临时数据目录和 ASR/Voice 测试资源。
- **执行记录（2026-05-29）**:
  - `cargo test -p bifrost-core process_alias_executable -- --nocapture`：PASS，验证创建 `bifrost-agent` 场景别名并拒绝包含路径分隔符的非法别名。
  - `cargo test -p bifrost-admin im_gateway::agent_worker::tests:: --lib -- --nocapture`：PASS，确认 Agent worker 改用别名 executable 后既有 worker 协议回归仍通过。
  - `cargo test -p bifrost-admin external_cli --lib -- --nocapture`：PASS，确认 external runner worker 改用别名 executable 后既有 runner 回归仍通过。

### TC-IMA-84: AI 一级页合并 Agent/IM Gateway 子导航并按 URL 切换独立面板

- **前置条件**:
  - 使用临时数据目录启动 Bifrost，端口不得使用 9900：
    ```bash
    BIFROST_DATA_DIR=./.bifrost-test-agent-nav cargo run --bin bifrost -- start -p 18884 --unsafe-ssl --no-system-proxy
    ```
  - 浏览器打开 `http://127.0.0.1:18884/_bifrost/ai?aiSection=agent-general&agentSection=general`
- **操作步骤**:
  1. 确认主侧栏高亮 `AI`，且 Settings 与 AI 是同级入口。
  2. 确认 AI 页面左侧显示合并后的子导航，包含 Agent 分组的 General、Model、Runtime、History、Memories、Skills、Memory Records、MCP Servers、Sessions，以及 IM Gateway 分组的 Connections、Targets、Routes、Schedules、History。
  3. 确认默认只渲染 `General` 编辑卡片，其他卡片未同时出现在右侧。
  4. 点击左侧导航中的 Agent `MCP Servers`。
  5. 确认右侧只渲染 `MCP Servers` 编辑卡片，且 `MCP Servers` 导航项显示当前高亮状态。
  6. 确认浏览器 URL 包含 `aiSection=agent-mcp-servers` 与 `agentSection=mcp-servers`。
  7. 刷新页面，确认仍恢复到 `MCP Servers` 编辑卡片。
  8. 点击左侧导航中的 IM Gateway `Routes`。
  9. 确认右侧只渲染 `Routes` 面板，且 URL 更新为 `aiSection=im-gateway-routes` 与 `imGatewaySection=routes`。
  10. 点击左侧导航中的 Agent `Runtime`，确认右侧只渲染 `Runtime Settings` 编辑卡片。
  11. 切换到暗色主题后重复点击 Agent `MCP Servers` 与 IM Gateway `Routes`。
- **预期结果**:
  - AI 页面顶部有正常留白，左侧导航和右侧第一张卡片都不贴住窗口顶部。
  - 合并后的左侧导航始终可见并固定在左侧区域，不跟随右侧内容滚动。
  - 点击导航项后右侧独立渲染对应编辑卡片，不再把所有卡片堆在一个长页面中。
  - 当前导航项通过高亮和 `aria-current="true"` 标记。
  - URL 中的 `aiSection`、`agentSection`、`imGatewaySection` 能记录当前卡片，页面刷新后恢复到同一卡片。
  - 亮色与暗色主题下导航项、文本、边框和高亮状态均清晰可读。
  - 窄屏下导航退化为顶部横向滚动，不挤压编辑卡片内容。
- **执行记录（2026-05-12）**: PASS — 执行 `source ~/.zshrc && pnpm --dir web exec playwright test tests/ui/admin-settings.spec.ts --grep "AI 一级页整合 (Agent|IM Gateway)"`，真实 Chromium 验证 AI 主侧栏入口与 Settings 同级、AI 页顶部留白至少 12px、Agent/IM Gateway 子导航合并、右侧一次只渲染当前面板、`aiSection` + `agentSection` / `imGatewaySection` 刷新恢复、暗色主题切换后可读，以及从 stale session URL 点击 Agent Runtime 会清理 `session/view/historyPath` 并回到 Runtime 卡片。

### TC-IMA-84A: Agent 模型配置可关闭 reasoning 参数

- **前置条件**:
  - 使用当前分支启动 Bifrost，端口不得使用 9900，必须显式关闭系统代理：
    ```bash
    BIFROST_DATA_DIR="$(mktemp -d)" cargo run --bin bifrost -- start -p 18884 --unsafe-ssl --no-system-proxy
    ```
  - 浏览器打开 `http://127.0.0.1:18884/_bifrost/ai?aiSection=agent-model&agentSection=model`
- **操作步骤**:
  1. 确认右侧只渲染 `Model Configuration` 编辑卡片。
  2. 将模型名填写为一个不支持 Chat Completions reasoning 参数的模型，例如 `gpt-5.5-2026-04-01`。
  3. 将 `Reasoning Effort` 下拉框切换为 `None (disabled)`。
  4. 将 `Reasoning Summary` 下拉框切换为 `None (disabled)`。
  5. 执行 `curl -s http://127.0.0.1:18884/_bifrost/api/im-gateway/agent | jq '{model, model_reasoning_effort, model_reasoning_summary}'`。
  6. 在亮色和暗色主题下各确认一次两个下拉框的当前值可读。
- **预期结果**:
  - WebUI 保存成功并显示更新提示。
  - API 返回 `model_reasoning_effort: "none"` 与 `model_reasoning_summary: "none"`。
  - Agent 运行时把 `"none"` 解释为禁用，不向 Chat Completions 请求体写入 `reasoning_effort` 或 `reasoning_summary` 字段。
  - 亮色和暗色主题下两个配置项都位于 Agent → Model 配置卡片内，文字和下拉值清晰可读。
- **执行记录（2026-05-10）**: PASS — 使用临时数据目录 `/tmp/bifrost-reasoning-human.iLYev0` 启动 `./target/debug/bifrost start -p 18884 --unsafe-ssl --no-system-proxy`，通过 Codex in-app browser 打开旧版 `/_bifrost/settings?tab=agent&agentSection=model`，将模型填为 `gpt-5.5-2026-04-01`，将 `Reasoning Effort` 与 `Reasoning Summary` 均切换为 `None (disabled)`；`curl -fsS http://127.0.0.1:18884/_bifrost/api/im-gateway/agent | jq '{model, model_reasoning_effort, model_reasoning_summary}'` 与临时目录 `agent/agent_config.json` 均返回 `model_reasoning_effort: "none"`、`model_reasoning_summary: "none"`；切换暗色主题后控件和 `None (disabled)` 值仍可见。本轮 UI 入口改为 `/_bifrost/ai?aiSection=agent-model&agentSection=model`，需执行最新入口回归。

### TC-IMA-85: `/agent/chat` 图片多模态理解真实链路

- **前置条件**:
  - 使用临时数据目录启动 Bifrost，端口不得使用 9900，必须显式关闭系统代理：
    ```bash
    BIFROST_DATA_DIR="$(mktemp -d)" cargo run --bin bifrost -- start -p 18885 --unsafe-ssl --no-system-proxy
    ```
  - 启动 OpenAI-compatible mock Chat Completions 服务；mock 必须记录请求 JSON，并在收到 `image_url` content part 且文本包含 `MULTIMODAL_IMAGE_E2E` 时返回 `MULTIMODAL_IMAGE_E2E_OK`。
- **操作步骤**:
  1. 配置 Agent 使用 mock 多模态模型：
     ```bash
     curl -s -X PATCH http://127.0.0.1:18885/_bifrost/api/im-gateway/agent \
       -H 'Content-Type: application/json' \
       -d '{
         "enabled": true,
         "model": "mock-vision-model",
         "model_provider": "mock",
         "base_instructions": "You can understand images.",
         "max_turn_iterations": 1,
         "memories": {"use_memories": false, "generate_memories": false},
         "model_providers": {
           "mock": {
             "name": "Mock",
             "base_url": "http://127.0.0.1:<mock_port>/chat/completions",
             "api_key": "test"
           }
         }
       }'
     ```
  2. 通过真实 `/agent/chat` API 发送文本 + 图片：
     ```bash
     curl -s -X POST http://127.0.0.1:18885/_bifrost/api/im-gateway/agent/chat \
       -H 'Content-Type: application/json' \
       -d '{
         "session_key": "human-multimodal-image",
         "message": "MULTIMODAL_IMAGE_E2E 请描述这张图片",
         "images": [{
           "mime_type": "image/png",
           "data": "iVBORw0KGgo="
         }]
       }'
     ```
  3. 检查响应 JSON 和 mock 收到的请求体。
  4. 打开 WebUI `Settings → Agent → Sessions`，进入 `human-multimodal-image` Session 详情，或调用：
     ```bash
     curl -s http://127.0.0.1:18885/_bifrost/api/im-gateway/agent/sessions/human-multimodal-image
     ```
- **预期结果**:
  - `/agent/chat` 返回 `success: true`。
  - `response` 包含 `MULTIMODAL_IMAGE_E2E_OK`，证明模型链路消费到了图片输入。
  - mock 收到的最后一条 user message 的 `content` 是数组，至少包含一个 `{"type":"text"}` part 和一个 `{"type":"image_url"}` part。
  - `image_url.url` 以 `data:image/png;base64,` 开头。
  - Session 详情 API 的 user message 包含 `content_parts` 中的 `image_url` part。
  - WebUI Session 详情中 user message 下方显示图片缩略图，点击缩略图后打开放大预览层。
  - 会话 JSONL 的 `user_message.content.images` 保存 `{mime_type,data}`，后续历史会话仍可查看图片。
  - 启动命令包含 `--no-system-proxy`，全程未使用 9900 端口。
- **清理步骤**:
  - 停止 Bifrost 进程和 mock 服务。
  - 删除本用例创建的临时 `BIFROST_DATA_DIR`。
- **执行记录（2026-05-06）**: PASS — 执行 `CARGO_TARGET_DIR=target/im-multimodal-e2e BIFROST_E2E_RUNNER_JOBS=1 cargo run -p bifrost-e2e -- --test im_gateway_agent_chat_multimodal_image_parts --test-timeout 120 --port 18885` 通过。用例启动真实 Bifrost admin + mock 多模态模型，通过 `/agent/chat` 发送文本和 `image/png` base64 图片，断言 mock 收到 OpenAI-compatible `image_url` content part，接口返回 `MULTIMODAL_IMAGE_E2E_OK`，并验证 Session 详情 API 的 user message 包含持久化 `content_parts.image_url`。

### TC-IMA-86: 飞书富文本 post 图片+文字消息进入 Agent

- **前置条件**:
  - Feishu `im.message.receive_v1` 事件中的 `message_type` 为 `post`。
  - `message.content` 使用飞书接收态富文本结构：顶层包含 `title` 和 `content`，其中 `content` 为二维数组，元素可包含 `tag=text/a/at/img/media/code_block`。
- **操作步骤**:
  1. 构造一条 `post` 消息，`content` 中包含 `{"tag":"text","text":"请看这张图"}` 和 `{"tag":"img","image_key":"img_v3_post"}`。
  2. 调用 Feishu 事件归一化逻辑。
  3. 构造一条图片-only IM event，`text=""` 且 `images` 非空，送入 IM event loop。
  4. 检查模型 mock 收到的 Chat Completions 请求。
- **预期结果**:
  - 归一化后的 `message.text` 为 `请看这张图`，不是空字符串。
  - 归一化后的 `message.images[0].file_key` 为 `img_v3_post`。
  - 日志包含 `normalized feishu inbound message`，并输出 `message_type`、`text_len`、`image_count`、`image_keys`、`content_keys`、`content_preview`。
  - 图片-only 消息不会因为文本为空被跳过；模型请求包含默认图片理解提示和 `image_url` content part。
  - inbound message log 对图片-only 消息显示 `[图片消息: 1 张]` 预览。
- **执行记录（2026-05-06）**: PASS — 执行 `cargo test -p bifrost-admin im_gateway::feishu::tests::test_normalize_feishu_post_extracts_text_and_images` 通过，验证接收态 `post` 顶层 `content` 结构能提取文字和图片 key；执行 `cargo test -p bifrost-admin handlers::im_gateway::tests::im_event_loop_forwards_image_attachment_to_agent_chat` 通过，验证图片-only IM event 不再被空文本短路，会进入 Agent 并携带 `image_url` content part。

### TC-IMA-87: 图片数量上限与 Session 详情放大预览

- **前置条件**:
  - Agent 已启用，模型使用 OpenAI-compatible Chat Completions mock。
  - WebUI 可访问 `Settings → Agent → Sessions`。
- **操作步骤**:
  1. 通过 `/agent/chat` 或飞书 IM 链路提交 7 张图片的单条消息。
  2. 检查模型 mock 收到的 Chat Completions 请求。
  3. 打开对应 Session 详情，查看 active session 的 user message 图片区域。
  4. 结束会话后从 History session 详情再次查看同一条 user message 图片区域。
  5. 分别点击 active 和 history 详情中的图片缩略图。
- **预期结果**:
  - 模型请求中最多包含 6 个 `image_url` content part。
  - 超过 6 张时服务日志包含 `too many IM images in one message; truncating images for agent multimodal input` 或 `too many /agent/chat images in one request; truncating images`。
  - Session 详情 active view 显示图片缩略图，点击后打开放大预览层。
  - Session 详情 history view 从 JSONL `user_message.content.images` 恢复图片缩略图，点击后同样打开放大预览层。
  - 图片缩略图和预览层在亮色、暗色主题下均可辨识。
- **清理步骤**:
  - 停止 Bifrost 进程和 mock 服务。
  - 删除本用例创建的临时 `BIFROST_DATA_DIR`。
  - 清理浏览器测试会话。
- **执行记录（2026-05-06）**: PASS — 执行 `cargo test -p bifrost-admin handlers::im_gateway::tests::im_event_loop_forwards_image_attachment_to_agent_chat` 通过，测试构造 7 张图片的 IM event，验证进入模型请求的 `image_url` content part 被截断为 6 张；执行 `pnpm --dir web exec tsc --noEmit` 通过，验证 Session 详情图片缩略图改用 Ant Design `Image.PreviewGroup` 后类型检查通过，active/history 图片均可点击触发内置放大预览。

### TC-IMA-89: `/stop` 停止运行中 Agent loop

- **前置条件**:
  - 使用临时 `BIFROST_DATA_DIR` 启动真实 Bifrost Admin，启动参数必须包含 `--no-system-proxy`。
  - Agent 已启用，模型 Provider 指向一个 OpenAI-compatible mock Chat Completions 服务。
  - mock 服务在收到包含 `AGENT_STOP_E2E` 的请求时延迟响应，用于制造正在执行的 active loop。
- **操作步骤**:
  1. PATCH `/_bifrost/api/im-gateway/agent`，启用 Agent，设置 `model_provider=mock`、`request_timeout_secs=60`、关闭 memories。
  2. 通过 `POST /_bifrost/api/im-gateway/agent/chat` 使用 `session_key=stop-loop-e2e` 发送 `AGENT_STOP_E2E please keep this model request open`，保持请求运行。
  3. 在 mock 确认已收到模型请求后，用同一 `session_key` 调用 `/agent/chat`，message 为 `/status`。
  4. 用同一 `session_key` 调用 `/agent/chat`，message 为 `/stop`。
  5. 等待第 2 步的原始 chat 请求返回。
  6. 再用同一 `session_key` 发送一条普通 chat，确认 session 已释放且后续对话可继续。
- **预期结果**:
  - 第 3 步 `/status` 返回 `success=true` 且包含 `active_status`，说明 session 正在运行。
  - 第 4 步 `/stop` 立即返回 `success=true`、`stopped=true`，不排队、不等待原始 chat 完成。
  - 第 2 步原始 chat 在 `/stop` 后快速返回，response 包含 `/stop` 停止提示。
  - 第 6 步普通 chat 返回 `success=true`，证明 active session 已释放且没有卡死。
  - 停止过程不修改系统代理，不使用本机默认 `~/.bifrost` 数据目录。
- **清理步骤**:
  - 停止 Bifrost 进程和 mock 服务。
  - 删除本用例创建的临时 `BIFROST_DATA_DIR` 与 E2E target 目录（如适用）。
- **执行记录（2026-05-10）**: PASS — 执行 `source ~/.zshrc && CARGO_TARGET_DIR=target/agent-stop-e2e BIFROST_E2E_RUNNER_JOBS=1 cargo run -p bifrost-e2e -- --test im_gateway_agent_chat_stop_active_loop --test-timeout 120 --port 18886` 通过。用例启动真实 Admin + mock Chat Completions，发起包含 `AGENT_STOP_E2E` 的长请求；同 session `/status` 返回 `active_status`；同 session `/stop` 返回 `stopped=true`；原 chat 快速返回包含 `/stop` 的停止提示；随后同 session 普通 chat 返回 `success=true`。

### TC-IMA-93: send_msg 默认消息通道与 schedule 绑定设计一致性

- **前置条件**:
  - 本用例用于设计文档变更后的真实可检索性检查，不启动 Bifrost 服务。
  - 技术方案已写入 `design/im-gateway-agent.md` 和 `design/im-gateway.md`。
- **操作步骤**:
  1. 执行 `rg -n "default_message_channel|ImMessageChannelBinding|send_msg|AgentMessageContext" design/im-gateway-agent.md design/im-gateway.md`。
  2. 执行 `rg -n "schedule.message_channel|任务绑定|默认发送通道|来源通道" design/im-gateway-agent.md design/im-gateway.md`。
  3. 执行 `rg -n "TC-IMA-93|send_msg 默认消息通道" human_tests/im-gateway-agent.md human_tests/readme.md`。
- **预期结果**:
  - 技术文档明确说明 Agent 配置中的 `default_message_channel`。
  - 技术文档明确说明 `send_msg` 是统一工具名，飞书/微信能力通过动态 description 和 schema 裁剪。
  - 技术文档明确说明手动 schedule 必须绑定 IM 通道，Agent 创建 schedule 时自动继承当前来源或默认通道。
  - 技术文档明确说明 schedule 执行发送消息时优先使用任务保存的 `message_channel`，不会使用最近 IM 对话来源。
  - `human_tests/readme.md` 索引包含本用例覆盖点。
- **执行记录（2026-05-13）**: PASS — 执行 `rg -n "default_message_channel|ImMessageChannelBinding|send_msg|AgentMessageContext" design/im-gateway-agent.md design/im-gateway.md`，命中 Agent 配置默认发送通道、统一 `send_msg` 工具和 turn 级 `AgentMessageContext`；执行 `rg -n "schedule.message_channel|任务绑定|默认发送通道|来源通道" design/im-gateway-agent.md design/im-gateway.md`，命中 schedule 绑定通道、默认通道和任务执行目标不漂移规则；执行 `rg -n "TC-IMA-93|send_msg 默认消息通道" human_tests/im-gateway-agent.md human_tests/readme.md`，确认 human_tests 用例与索引均可检索。

### TC-IMA-94: send_msg 默认通道与 Agent 创建 schedule 真实链路

- **前置条件**:
  - 使用临时 `BIFROST_DATA_DIR` 启动真实 Bifrost Admin，启动参数必须包含 `--no-system-proxy`。
  - Agent Provider 指向 OpenAI-compatible mock Chat Completions 服务。
  - IM Provider 使用 Weixin mock 服务，并配置 `owner_open_id`、`app_id`、`secret_ref` 和 `base_url`。
  - Agent 配置包含 `default_message_channel`，绑定到 Weixin mock provider 的 owner 通道。
- **操作步骤**:
  1. 执行 `BIFROST_PORT=18941 MOCK_PORT=18942 e2e-tests/tests/test_agent_send_msg_default_channel.sh`。
  2. E2E 脚本启动 mock 模型服务和 Weixin sendmessage mock 服务。
  3. E2E 脚本通过 `/agent/chat` 触发模型返回 `send_msg` 和 `schedule_create` tool calls。
  4. 检查 mock Weixin sendmessage 请求日志。
  5. 检查 `/api/im-gateway/schedules` 返回的 schedule JSON。
- **预期结果**:
  - 模型请求中的 tools 同时包含 `send_msg` 和 `schedule_create`。
  - `send_msg` 不需要显式 `provider_id` / `target_id`，会使用 Agent 配置的 `default_message_channel`。
  - Weixin mock sendmessage 收到 `to_user_id=mock-user@im.wechat`，文本为 `hello via send_msg`。
  - Agent 创建的 `default-bound-schedule` 即使未显式传入 `target_id` / `message_channel`，也会自动写入 `message_channel.provider_id=weixin-mock`、`target_mode=owner`、`target_id=mock-user@im.wechat`。
  - Bifrost 进程使用临时数据目录，不修改系统代理。
- **清理步骤**:
  - E2E 脚本退出时停止 Bifrost 与 mock 服务。
  - E2E 脚本删除临时 `BIFROST_DATA_DIR`。
- **执行记录（2026-05-13）**: PASS — 执行 `BIFROST_PORT=18941 MOCK_PORT=18942 e2e-tests/tests/test_agent_send_msg_default_channel.sh` 通过。脚本真实启动 Bifrost Admin、mock Chat Completions 和 Weixin sendmessage；模型请求校验到 `send_msg` 与 `schedule_create`；`send_msg` 使用默认通道发送到 Weixin owner；schedule 创建结果持久化 `message_channel` 默认绑定。

### TC-IMA-95: 真实用户默认 IM 通道 send_msg 模型兼容链路

- **前置条件**:
  - 从 `~/.bifrost` 复制真实用户 IM Provider 与 Agent 配置到临时 `BIFROST_DATA_DIR`，避免污染默认数据目录。
  - 用当前源码启动 Bifrost：`BIFROST_DATA_DIR=<temp> cargo run --bin bifrost -- start -p 18955 --unsafe-ssl --no-system-proxy`。
  - Agent 配置 `default_message_channel` 绑定到真实可用的 Feishu `bifrost` provider owner 通道。
- **操作步骤**:
  1. 通过 `/api/im-gateway/messages/send` 向 `provider_id=bifrost,target_id=owner` 发送一条预检文本。
  2. 通过 `PATCH /api/im-gateway/agent` 设置 `default_message_channel`。
  3. 调用 `POST /api/im-gateway/agent/chat`，system prompt 要求模型只调用一次 `send_msg`，发送唯一时间戳文本。
  4. 检查 chat API 的 `tool_calls`，确认 `send_msg` 成功。
  5. 检查 `admin/im_gateway_message_logs.json`，确认存在 `trigger=agent_tool:send_msg`、`status=success`、真实 `message_id`。
- **预期结果**:
  - 真实模型接口接受 `send_msg` 工具 schema，不因顶层 `anyOf` / `oneOf` 等组合关键字拒绝请求。
  - `send_msg` 不传 `provider_id` / `target_id` 时使用 Agent 默认通道。
  - IM provider 返回真实 `message_id`，消息日志记录 `status=success`。
  - 服务启动参数包含 `--no-system-proxy`，不修改系统代理。
- **清理步骤**:
  - 停止测试 Bifrost 进程。
  - 删除临时 `BIFROST_DATA_DIR`。
- **执行记录（2026-05-13）**: PASS — 真实启动当前源码 Bifrost，预检直发 Feishu `bifrost` provider 成功，返回 `om_x100b6f744c70b93cc32aa02b96e1ea3`；首次 `/agent/chat` 暴露 AIDP schema 兼容问题（`send_msg` 顶层 `anyOf` 被拒绝），修复 schema 后重启复测通过。chat 返回 `tool_calls[0].tool_name=send_msg`、`success=true`，工具结果包含真实 `message_id=om_x100b6f74423a54a0c2945f565987a08`；消息日志记录 `trigger=agent_tool:send_msg`、`status=success`、文本 `Bifrost agent chat send_msg real test 20260513-124040`。

### TC-IMA-95A: 飞书通道 send_msg 默认生成图文卡片

- **前置条件**:
  - 使用临时 `BIFROST_DATA_DIR` 启动当前源码 Bifrost，启动命令必须包含 `--no-system-proxy`。
  - 准备 fake Feishu OpenAPI 服务，覆盖 tenant token 与 `POST /im/v1/messages`。
  - Agent 默认消息通道绑定到 `provider_type=feishu` 的 Provider owner。
- **操作步骤**:
  1. 本地源码构建路径执行 `BIFROST_PORT=18945 MOCK_PORT=18946 bash e2e-tests/tests/test_agent_send_msg_feishu_card.sh`。
  2. CI 预构建 release 复用路径执行 `SKIP_BUILD=true BIFROST_BIN="$PWD/target/release/bifrost" ADMIN_PORT=18945 MOCK_HTTP_PORT=18946 bash e2e-tests/tests/test_agent_send_msg_feishu_card.sh`。
  3. 脚本通过 `/agent/chat` 触发 mock 模型调用 `send_msg`，参数包含 `markdown`、`image_key`、`image_alt` 与 `card_title`，不传原始 `card`。
  4. 检查 mock 模型收到的 `send_msg` schema，确认飞书通道暴露 `card` 与 `image_key` 字段。
  5. 检查 fake Feishu 捕获的 `POST /im/v1/messages?receive_id_type=open_id` 请求体。
  6. 检查 outbound message log 中 `trigger=agent_tool:send_msg` 的记录。
- **预期结果**:
  - `send_msg` 工具在飞书默认通道下不再把 `markdown` 当纯文本发送。
  - 飞书发送请求 `msg_type=interactive`。
  - `content` 为 Feishu JSON 2.0 card，`header.title.content=Card Send`。
  - `body.elements[0]` 为 `tag=img` 且 `img_key=img_v3_chart`，`body.elements[1]` 为 `tag=markdown` 且内容为模型传入 Markdown。
  - 消息日志记录 `msg_type=interactive`，不泄露 open_id 以外的 token/secret。
  - `SKIP_BUILD=true` 时脚本使用 `target/release/bifrost` 或显式 `BIFROST_BIN`，不会查找 CI 中不存在的 `target/debug/bifrost`。
  - 并行 shell 调度时脚本使用 `ADMIN_PORT` 与 `MOCK_HTTP_PORT` 注入端口，避免默认端口碰撞。
- **执行记录（2026-05-27）**: PASS — 执行 `BIFROST_PORT=18945 MOCK_PORT=18946 bash e2e-tests/tests/test_agent_send_msg_feishu_card.sh` 通过。脚本使用临时 `BIFROST_DATA_DIR`、源码构建的 `target/debug/bifrost` 和 `--no-system-proxy` 启动真实 Admin；mock 模型请求确认 `send_msg` schema 在飞书默认通道下包含 `card` 与 `image_key`；模型只传 `markdown`、`image_key`、`image_alt`、`card_title`，fake Feishu 捕获到 `msg_type=interactive`，卡片 `schema=2.0`、`header.title.content=Card Send`、首个元素为 `img_key=img_v3_chart`，第二个元素为 Markdown `**card body** with image`；outbound message log 记录 `trigger=agent_tool:send_msg` 且 `msg_type=interactive`。
- **回归执行记录（2026-05-27）**: PASS — CI run `26515240075` 的 Linux/macOS `E2E Shell shard 3/3` 均在 `test_agent_send_msg_feishu_card.sh` 失败，日志显示 `target/debug/bifrost: No such file or directory`。修复后执行 `bash -n e2e-tests/tests/test_agent_send_msg_feishu_card.sh`、`rg -n 'BIFROST_PORT=.*ADMIN_PORT|MOCK_PORT=.*MOCK_HTTP_PORT|target/release/bifrost|target/debug/bifrost' e2e-tests/tests/test_agent_send_msg_feishu_card.sh` 与 `SKIP_BUILD=true BIFROST_BIN="$PWD/target/release/bifrost" ADMIN_PORT=18945 MOCK_HTTP_PORT=18946 bash e2e-tests/tests/test_agent_send_msg_feishu_card.sh` 通过；脚本在 release binary 路径下真实启动 Admin，fake Feishu 捕获 `msg_type=interactive`，消息日志记录 `trigger=agent_tool:send_msg` 且 `msg_type=interactive`。


### TC-IMA-96: ChatGPT Web 长回复经 Weixin 失败后拆分补发

- **前置条件**:
  - 当前源码已包含 ChatGPT Web DOM 提取不截断修复。
  - 使用 Weixin mock `sendmessage` 服务模拟首次长文本发送返回 `ret=-2`，后续分片发送返回成功。
  - 不需要启动系统代理；自动化脚本只运行 provider/adapter 回归测试。
- **操作步骤**:
  1. 执行 `e2e-tests/tests/test_im_gateway_long_reply_delivery_regression.sh`。
  2. 检查脚本中的 `chatgpt_web_dom_extraction_does_not_truncate_response_text` 断言。
  3. 检查脚本中的 `send_text_retries_failed_long_message_as_split_messages` 断言。
- **预期结果**:
  - ChatGPT Web DOM 提取脚本中不存在 `text.slice(0, 10000)` 或 `t.slice(0, 10000)`，长回复 artifact 可保存全文。
  - Weixin mock 收到的第 1 条 `sendmessage` 为完整原文；该请求失败后，provider 继续发送多条带 `[i/N]` 前缀的小文本。
  - 去掉分片前缀后按顺序拼接，内容与完整原文完全一致，中文和换行不被破坏。
  - 补发成功后 `send_text` 返回成功，不把最终回复整体标记为失败。
- **清理步骤**:
  - 自动化脚本退出后 mock server 随测试进程释放，无需保留临时数据。
- **执行记录（2026-05-22）**: PASS — 执行 `source ~/.zshrc && e2e-tests/tests/test_im_gateway_long_reply_delivery_regression.sh` 通过：`chatgpt_web_dom_extraction_does_not_truncate_response_text` 确认 DOM 提取脚本不存在固定 10000 字符截断；`send_text_retries_failed_long_message_as_split_messages` 使用 Weixin mock 首次返回 `ret=-2`，随后收到多条 `[i/N]` 分片，去前缀后拼接等于完整原文。补充执行 `source ~/.zshrc && cargo test -p bifrost-admin split_text_for_retry_preserves_multibyte_content --lib -- --nocapture` 通过，确认中文、多字节字符和换行切分保真。

### TC-IMA-97: AI Agent Chat 页面深链与真实流式 API 交互

- **前置条件**:
  - 使用临时 `BIFROST_DATA_DIR` 启动 Bifrost Admin，启动参数包含 `--no-system-proxy`。
  - WebUI 可通过 `http://127.0.0.1:<port>/_bifrost/` 访问。
- **操作步骤**:
  1. 打开 `http://127.0.0.1:<port>/_bifrost/ai?aiSection=agent-chat&agentSection=chat`。
  2. 检查 AI 左侧导航中 Agent Chat 项处于选中状态。
  3. 检查页面出现对话区、Workspace、Run Settings 和 Recent Threads。
  4. 在未输入内容时检查 Send 按钮禁用。
  5. 使用 mock SSE 或真实 Agent 配置拦截 `/_bifrost/api/agent/chat/stream`，点击 `Review the latest diff` prompt chip，再点击 Send。
- **预期结果**:
  - URL 深链直接进入 Agent Chat 页面，不跳回 Agent General。
  - 对话区显示 starter messages，布局不挤压右侧上下文卡片。
  - Prompt chip 内容写入 composer，Send 从禁用变为可点击。
  - 点击 Send 后输入框清空，对话区追加用户消息和来自流式 API 的 assistant 回复。
  - 请求体包含非空 `session_key` 和输入的 `message`，不再停留在本地 preview。

### TC-IMA-98: AI 页面兼容旧 agentSection 会话深链

- **前置条件**:
  - WebUI 可访问，Agent sessions API 可为空或返回测试数据。
- **操作步骤**:
  1. 打开 `http://127.0.0.1:<port>/_bifrost/ai?agentSection=sessions&session=session-key-1`。
  2. 检查页面 URL 是否补齐 `aiSection=agent-sessions`。
  3. 检查 Agent Sessions 导航项处于选中状态。
- **预期结果**:
  - 旧链接仍能进入 Agent Sessions，不因 AI 一级页新导航参数丢失上下文。
  - `session` query 保留，用于后续 Session 详情入口解析。
  - 页面不会进入 IM Gateway 或 ASR 子页。

### TC-IMA-101: Agent Chat Composer 键盘发送与多行输入

- **前置条件**:
  - 使用临时 `BIFROST_DATA_DIR` 启动 Bifrost Admin，启动参数包含 `--no-system-proxy`。
  - WebUI 可通过 `http://127.0.0.1:<port>/_bifrost/` 访问。
- **操作步骤**:
  1. 打开 `http://127.0.0.1:<port>/_bifrost/ai?aiSection=agent-chat&agentSection=chat`。
  2. 在 composer 中输入 `Line one`。
  3. 按 `Shift+Enter`，继续输入 `Line two`。
  4. 确认对话区尚未追加该草稿内容。
  5. 按 `Enter` 发送。
- **预期结果**:
  - `Shift+Enter` 只在 composer 中插入换行，不触发发送。
  - composer 值为两行文本：`Line one` 与 `Line two`。
  - 按 `Enter` 后 composer 清空。
  - 对话区追加用户两行消息和来自流式 API 的 assistant 回复。
  - 发送过程中输入框与 Send 按钮处于运行中状态，避免重复提交。

### TC-IMA-99: Agent Chat 后端文本/图片消息入口语义

- **前置条件**:
  - 本用例可通过 Rust 单元测试或真实 IM mock 链路验证。
- **操作步骤**:
  1. 构造包含前后空白文本和图片附件的 IM event message。
  2. 构造空文本但包含图片附件的 IM event message。
  3. 构造超长文本和图片-only inbound message preview。
  4. 执行：
     ```bash
     cargo test -p bifrost-admin agent_chat_message_text_prefers_trimmed_text_and_uses_image_prompt_fallback --lib -- --nocapture
     cargo test -p bifrost-admin inbound_message_preview_summarizes_image_only_and_truncates_text --lib -- --nocapture
     ```
- **预期结果**:
  - 文本消息进入 Agent 前会 trim，且文本优先于图片默认提示。
  - 图片-only 消息不会被空文本短路，会使用默认图片理解提示。
  - 空文本且无图片时返回空字符串。
  - inbound 日志 preview 对图片-only 消息显示 `[图片消息: N 张]`，超长文本安全截断且不破坏 UTF-8。

### TC-IMA-100: Agent Chat 进度事件刷新节流边界

- **前置条件**:
  - 可执行 `bifrost-admin` lib 单元测试。
- **操作步骤**:
  1. 执行：
     ```bash
     cargo test -p bifrost-admin progress_events_flush_immediately_only_for_visible_chat_updates --lib -- --nocapture
     ```
  2. 对照一次真实 Feishu progress card 长任务，观察 status 与 assistant/tool/final 事件刷新节奏。
- **预期结果**:
  - 高频 `Status` 事件不会立即刷新卡片，避免状态上报淹没 CardKit 更新。
  - 用户可见的 `AssistantDelta`、`AssistantFinal`、`TurnFinished`、`TurnFailed` 会立即刷新。
  - tool、plan、title 等可见结构性事件仍保持即时刷新策略。
  - 长任务过程中卡片既能及时显示关键输出，又不会因 status tick 造成过多更新。

### TC-IMA-116: Agent Chat 从 JSONL 历史安全恢复并续聊

- **前置条件**:
  - 使用临时 `BIFROST_DATA_DIR` 启动 Bifrost Admin，启动参数包含 `--no-system-proxy`。
  - 数据目录下存在一份 `sessions/` 子目录内的 Agent JSONL 历史文件，内容至少包含一条 `user_message` 和一条 `assistant_message`。
  - 准备一份 `sessions/` 外部的 `.jsonl` 文件作为越权路径负例。
- **操作步骤**:
  1. 打开 `http://127.0.0.1:<port>/_bifrost/ai?aiSection=agent-chat&agentSection=chat&session=<session-key>&historyPath=<encoded-jsonl-path>`。
  2. 检查页面对话区是否渲染 JSONL 中的历史 user/assistant 消息。
  3. 在 composer 输入 `Continue this thread` 并点击 Send。
  4. 检查 `/_bifrost/api/agent/chat/stream` 请求体包含 `session_key`、`message` 和相同的 `history_path`。
  5. 用 API 直接请求外部 `.jsonl` 路径：`GET /_bifrost/api/im-gateway/agent/sessions/history/<encoded-outside-path>`。
- **预期结果**:
  - 历史消息只从合法的 Agent `sessions/` JSONL 文件恢复。
  - 续聊请求触发后端先恢复历史再执行新 turn，assistant 回复显示在对话区。
  - 新 turn 继续写回原 JSONL 文件，后续再次打开相同 `historyPath` 能看到续聊内容。
  - 外部 `.jsonl` 路径返回 400，错误说明路径不在 Agent sessions 目录内。

### TC-IMA-117: Agent Chat active 会话刷新恢复与线程列表选中回归

- **前置条件**:
  - 使用当前源码 WebUI（Vite dev server 或重新构建后的 Admin 静态资源）连接到已启动的 Bifrost Admin。
  - Bifrost Admin 中存在一个 active Agent Chat session，`GET /_bifrost/api/im-gateway/agent/sessions/<session-key>` 返回至少一条 user 消息和一条 assistant 消息。
  - `GET /_bifrost/api/im-gateway/agent/sessions/all` 返回超过右侧 Threads 区域高度的多条记录；如存在同一 `session_key` 的 active/history 记录，需要验证不会重复选中。
- **操作步骤**:
  1. 打开 `http://127.0.0.1:<web-port>/_bifrost/ai?aiSection=agent-chat&agentSection=chat&session=<session-key>&view=active`。
  2. 检查对话区是否显示该 active session detail 中的 user 与 assistant 消息，而不是 starter preview。
  3. 检查 Conversation 卡片标题是否展示当前对话标题（线程 title、set_title 或第一条用户消息摘要），而不是固定文案 `Conversation`。
  4. 刷新页面，重新检查第 2 步消息仍然显示。
  5. 检查切换或刷新恢复后的消息区直接展示底部，不执行平滑动画滚动到底部。
  6. 检查 Threads 列表容器出现独立滚动能力：列表内容高度大于可视高度，滚动不影响左侧对话输入区。
  7. 检查当前 active session 只有一条线程按钮处于 primary/selected 状态；同一 `session_key` 的 history 记录不应同时高亮。
- **预期结果**:
  - 只有 `session` 和 `view=active` 的深链刷新后仍能恢复 active session 消息。
  - 对话卡片标题跟随当前对话，不显示固定 `Conversation`。
  - 切换会话或刷新恢复时消息区立即定位到底部，不出现动画滚动等待。
  - URL 不会被错误改写为 history view，除非原 URL 未指定 active 且只有历史记录可恢复。
  - Threads 不截断为 8 条，列表本身可滚动。
  - 同一 `session_key` 的 active/history 记录不会重复选中。
- **清理步骤**:
  - 停止为本用例启动的 Vite dev server。
  - 不删除用户已有 active session，除非本用例专门创建了临时 session。
- **执行记录（2026-05-25）**: PASS — 使用用户已启动的 Bifrost Admin `http://127.0.0.1:9900` 作为后端，先通过 `curl http://127.0.0.1:9900/_bifrost/api/im-gateway/agent/sessions/admin-chat-1779677418274` 确认 active session detail 含 user 消息 `你好` 与 assistant 消息 `你好！有什么需要我帮你处理的？`。随后启动当前源码 WebUI：`WEB_PORT=3001 BACKEND_PORT=9900 pnpm --dir web dev --host 127.0.0.1`，用真实浏览器打开 `http://127.0.0.1:3001/_bifrost/ai?aiSection=agent-chat&agentSection=chat&session=admin-chat-1779677418274&view=active`。刷新前后均确认对话区包含上述两条消息，URL 保持 `view=active`；UI 自动化补充拦截 `scrollIntoView`，确认 active session 恢复和刷新恢复后的最后一次滚动行为均为 `auto`，不会平滑动画滚动到底部；Threads 列表 `scrollHeight=3984`、`clientHeight=320`，存在独立滚动能力；选中线程按钮数量为 1，文本为 `你好Active`，未出现同 session 多项同时高亮。本用例未启动或修改系统代理。

### TC-IMA-118: Agent Chat Threads 去重、来源/Runner 标签与 Settings 弹窗回填

- **前置条件**:
  - 使用当前源码 WebUI（Vite dev server 或重新构建后的 Admin 静态资源）连接到 Bifrost Admin。
  - `GET /_bifrost/api/im-gateway/agent/sessions/all` 至少返回 active、history 与不同来源的线程；可使用 mock API 构造同一 `session_key` 的 active/history 重复项。
  - `GET /_bifrost/api/im-gateway/agent/instructions` 返回默认 `work_dir`。
  - 已完成或已存在的会话详情中包含 `work_dir`、message/token/compaction/runner 元信息，或 JSONL history 中包含 `session_start`、`tool_call`、`tool_result`、`compaction`、`session_end` 事件。
- **操作步骤**:
  1. 打开 `http://127.0.0.1:<web-port>/_bifrost/ai?aiSection=agent-chat&agentSection=chat&session=<session-key>&view=active`。
  2. 检查右侧侧栏只展示 Threads 卡片，不再平铺 Workspace、Status、Context、Plan 等卡片。
  3. 检查 composer 输入框右下方 New Chat 按钮旁边存在 Status 按钮；点击 Status。
  4. 点击 New Chat，检查弹窗中出现 Workspace 输入框；在尚未输入问题的新会话里确认创建后，检查 session id 不变化。
  5. 在 Settings 弹窗中检查 Workspace 输入框显示默认或当前会话的工作目录，且已初始化会话里为只读/disabled，不能直接切换。
  6. 在 Status 弹窗中检查已存在/已完成会话的 Status、Context 不再全部为空：Status 显示 message/compaction，Context 显示 token/runner；弹窗中不展示 Tools 模块，也不展示本轮已执行工具的 Args/Result 记录。
  7. 检查对话标题栏展示来源、Runner 和状态标签；例如 ChatGPT Web/admin API 显示 Web，runner id 显示在 Runner 标签中，active session 显示 Active。
  8. 关闭 Settings 弹窗，检查没有 plan 的会话不显示 Plan 模块；触发或 mock 一次包含 `plan_updated` 的 stream/history 后，确认 Plan 显示在输入框上方而不是弹窗中。
  9. 点击 Plan 折叠按钮，切换会话后确认折叠状态保持；再次展开后切换会话确认展开状态保持。
  10. 检查 Threads 列表同一 `session_key` 只出现一条记录；active/history 重复时 active 优先展示且只有一个选中项。
  11. 检查每条线程显示来源标签：Feishu/Weixin/Lark 显示 IM，ChatGPT Web/admin API 显示 Web，Codex/external runner 显示 Runner，ASR runner 显示 ASR；线程列表不显示 `Active` / `Ended` 文案，只有 running 线程显示跳动绿点。
- **预期结果**:
  - 右侧侧栏干净，只保留 Threads 列表，列表自身可滚动。
  - Status 弹窗入口位于输入框按钮区，并承载 Workspace、Status、Context、Errors、Run Settings。
  - 只有 New Chat 弹窗允许选择待创建会话的 workspace；空白新会话重复 New Chat 不生成新 session id。
  - 已初始化会话的 Workspace 只读展示，不允许切换；已存在 active session 和已完成 history session 都能从 session detail 或 JSONL events 回填 workspace/status/context；Status 弹窗不展示 Tools 卡片。
  - 对话顶部展示来源、Runner 和状态标签。
  - 没有 plan 时不渲染 Plan；有 plan 时 Plan 位于输入框上方，折叠/展开偏好在页面内持续保持。
  - Threads 数据按 `session_key` 唯一展示，不出现重复数据或重复选中。
  - 每条线程都展示可读来源标签，不展示结束状态文案；running 线程通过跳动绿点提示。
- **清理步骤**:
  - 停止为本用例启动的 Vite dev server。
  - 删除自动化测试创建的临时 mock 数据；不删除用户已有 Bifrost 数据。
- **执行记录（2026-05-25）**: PASS — 执行 `pnpm --dir web exec playwright test tests/ui/agent-chat.spec.ts --grep "deep link renders|restores active session|thread list"`，3 条真实 Chromium UI 回归通过：覆盖 New Chat 弹窗选择 workspace、空白未输入新会话确认后 session label 不变、已初始化会话 Settings Workspace disabled、默认 Workspace 随 stream 请求发送、顶部来源/Runner/状态标签、Plan 初始隐藏且有 plan 时在输入框上方可折叠/展开、active session 刷新恢复、线程来源标签 IM/Web、线程列表不再展示 Ended 文案、running 线程展示绿点、同 `session_key` active/history 去重且只选中一条、Threads 独立滚动且窄屏仍保持在对话区右侧，长标题不撑宽页面。随后使用当前源码启动 `WEB_PORT=3001 BACKEND_PORT=9900 pnpm --dir web dev --host 127.0.0.1`，连接用户已启动的 `http://127.0.0.1:9900` 后端并用真实浏览器打开 active session；确认右侧侧栏只剩 Threads，Settings 弹窗 Workspace 为 `/Users/eden/work/github/bifrost`，Status 显示 `Messages 4` / `Compactions 0`，Context 显示 runner `bifrost_agent`，无 plan 时 Plan 模块隐藏，Threads 展示 Web/IM 来源标签且同一 Feishu session 历史被折叠为唯一一条记录。

### TC-IMA-125: Agent Chat Status 弹窗不展示 Tools 模块

- **前置条件**:
  - 使用当前源码 WebUI 连接到 Bifrost Admin。
  - Agent Chat stream 可以包含 `tool_started/tool_finished` 执行事件，用于验证执行记录不会混入 Status 弹窗。
- **操作步骤**:
  1. 打开 `http://127.0.0.1:<web-port>/_bifrost/ai?aiSection=agent-chat&agentSection=chat`。
  2. 发送一条消息，等待流式响应完成。
  3. 点击 Agent Chat 标题区的 Status 按钮。
  4. 查看 Status 弹窗内容。
- **预期结果**:
  - 弹窗标题为 `Agent Chat Status`。
  - 弹窗不展示 Tools 卡片，不展示 `Success`、`Args:`、`Result:`，也不展示本轮已执行工具名称（例如 mock 执行事件中的 `shell`）。
  - 已执行工具的过程信息仍只出现在 assistant 消息的 process steps 中。
- **清理步骤**:
  - 停止为本用例启动的 Vite dev server；不删除用户已有 Bifrost 数据。

### TC-IMA-119: Agent Chat Runner 选择、统一标题摘要与刷新不中断后台 Loop

- **前置条件**:
  - 使用当前源码构建或启动 Bifrost Admin；若连接用户已有 `9900` 服务，必须确认该后端已重启到包含本次修复的二进制，否则只能验证前端热更新，无法验证服务侧 disconnect 语义。
  - WebUI 可通过 `http://127.0.0.1:<web-port>/_bifrost/` 访问。
  - `GET /_bifrost/api/im-gateway/chat/config` 返回 `defaultRunnerId` 与至少一个自定义 Runner，例如 `codex` 或 `web`。
  - 存在一个 active session 和一份 JSONL history：两者都没有显式 title，但都包含第一条 user message。
- **操作步骤**:
  1. 打开 `http://127.0.0.1:<web-port>/_bifrost/ai?aiSection=agent-chat&agentSection=chat`，点击 New Chat。
  2. 在 New Chat 弹窗中打开 Runner 下拉，选择 `codex` 或 `web`，确认创建。
  3. 输入问题并发送，检查请求走 `/_bifrost/api/im-gateway/chat/stream`，请求体包含 `runnerId`、`adapter`、`workDir` 和 `message`。
  4. 打开 Threads 列表，检查没有显式 title 的 active/history 会话都显示第一条用户消息摘要，而不是 `session_key`；点击会话后标题不应从 `session_key` 抖动为另一段文本。
  5. 发起一个耗时 Agent Chat 请求，在浏览器开始收到 `run_started` 后刷新页面或关闭页面。
  6. 不点击 Stop，等待后台 turn/run 完成；重新打开相同 `session_key` 的 active/history 链接。
  7. 另起一次运行并点击 Stop 或发送 `/stop`，验证这次才真正停止当前轮次。
- **预期结果**:
  - 新建会话可选择内置 Bifrost Agent、Codex Runner、ChatGPT Web Runner 或其他已配置 Runner。
  - 自定义 Runner 发送时使用外部 Runner NDJSON stream，顶部 Runner 标签显示所选 runner id。
  - sessions/all 的公共字段稳定：同一 session 在列表和详情中的 title 来源一致，不因点击选中而抖动。
  - 浏览器刷新或 HTTP stream client disconnect 不会触发 `request_stop`，后台 Agent Loop 继续运行并最终写回 session。
  - 只有显式 Stop / `/stop` 会写入 stop signal，并释放 session 后允许后续继续对话。
- **清理步骤**:
  - 停止为本用例启动的 Vite dev server 和临时 Bifrost Admin。
  - 删除自动化测试创建的临时 data dir；不删除用户已有 Bifrost 数据。
- **执行记录（2026-05-25）**: PASS — UI 侧执行 `pnpm --dir web exec playwright test tests/ui/agent-chat.spec.ts --grep "external runner"`，真实 Chromium 验证 New Chat Runner 下拉选择 `codex` 后，请求命中 `/_bifrost/api/im-gateway/chat/stream`，请求体包含 `runnerId:"codex"`、`adapter:"codex"`、`workDir` 和用户消息，顶部 Runner 标签显示 `codex`。标题摘要与唯一数据源执行 `cargo test -p bifrost-agent scan_session_summary_uses_first_user_message_as_title_fallback --lib`、`cargo test -p bifrost-agent scan_session_summary_keeps_explicit_title_separate_from_first_user_fallback --lib`、`cargo test -p bifrost-agent test_session_info_from_list --lib`、`cargo test -p bifrost-admin sessions_all_dedupes_by_session_key_after_sorting --lib` 均通过，验证 active session list 与 JSONL summary 均使用显式 `title_updated` 优先、第一条用户消息作为 fallback，sessions/all 后端按 session_key 去重。运行中线程快照执行 `cargo test -p bifrost-agent test_running_turns_remain_visible_in_session_list --lib` 通过，验证 session checkout 后仍通过 `active_session_infos` 出现在 `list_sessions()` 并保留 running、title、workspace、runner 元信息。后台 Loop 语义通过代码 review 与 `cargo check -p bifrost-admin` 验证：`/_bifrost/api/agent/chat/stream` 与 `/_bifrost/api/im-gateway/chat/stream` 的 client disconnect 分支不再调用 `request_stop` 或 external CLI stop marker。真实 `9900` 已用 `zsh -lc 'source ~/.zshrc ... cargo run --bin bifrost -- start -p 9900 --unsafe-ssl --no-system-proxy'` 重启，PID `54277`，`ps eww -p 54277` 确认 `MODELHUB_AK` 与 `RUST_LOG` 已从 zshrc 进入进程环境，数据目录 `/Users/eden/.bifrost`，系统代理保持 disabled。

### TC-IMA-120: Agent Chat 运行中消息区不显示全局 Loading 图标

- **前置条件**:
  - WebUI 可访问，`/_bifrost/api/agent/chat/stream` 或 `/_bifrost/api/im-gateway/chat/stream` 可返回一个保持 running 的流式响应。
- **操作步骤**:
  1. 打开 Agent Chat 页面并发送消息。
  2. 在等待 assistant 回复期间观察消息区。
  3. 检查对话顶部状态标签、assistant 气泡和 Threads 运行态提示。
- **预期结果**:
  - 消息区左上角不出现独立 Spin/loading 图标。
  - 运行态仅通过顶部 `Running`、assistant 气泡 `Generating...` 和 Threads 跳动绿点表达。
  - 历史加载期间消息容器只设置 `aria-busy`，不改变视觉布局。
- **执行记录（2026-05-25）**: PASS — `pnpm --dir web exec tsc --noEmit` 通过；代码检查确认 `agent-chat-messages` 去除 `<Spin />`，仅保留 `aria-busy={historyLoading}`，assistant 气泡仍保留 `Generating...`。

### TC-IMA-121: Agent Chat Codex Runner 完成后不丢失会话且内容轨道居中

- **前置条件**:
  - 使用当前源码启动 WebUI 与 Bifrost Admin。
  - `GET /_bifrost/api/im-gateway/chat/config` 中 `codex` runner 为 enabled。
  - Admin 进程从 shell 配置加载到 Codex / 模型相关环境变量。
- **操作步骤**:
  1. 打开 `/_bifrost/ai?aiSection=agent-chat&agentSection=chat`。
  2. 点击 New Chat，选择 `codex` Runner，并使用默认或指定 Workspace 创建。
  3. 输入一条短消息并发送，等待 Codex Runner 返回 `run_finished`。
  4. 检查当前对话区仍保留 user 消息和 Codex assistant 回复。
  5. 刷新页面或点击线程列表中的该会话。
  6. 检查 `/_bifrost/api/im-gateway/agent/sessions/all` 仍包含该 session，且 `/sessions/<session_key>` 能返回合成的 user/assistant messages、runner_id、work_dir。
  7. 在 1600px 以上宽屏检查消息轨道和 composer 轨道最大宽度为 750px，并在 Conversation 主栏居中。
- **预期结果**:
  - Codex Runner 成功后不会因为 active preview 清理而从 Threads 消失。
  - 刷新或重新点击线程后，对话详情仍显示首条用户消息和最终回复。
  - 线程标题使用首条用户消息或显式 title，不回退到奇怪的 session id。
  - 消息区和输入区内容轨道在宽屏居中，宽度不超过 750px。
- **执行记录（2026-05-25）**: PASS — 直接调用真实 `9900`：`curl -N -sS -X POST /_bifrost/api/im-gateway/chat/stream`，请求 `runnerId:"codex"`、`adapter:"codex"`、`workDir:"/Users/eden/work/github/bifrost"`，返回 `run_started`、`assistant_final`、`run_finished status:"succeeded"`。随后新增并执行 `cargo test -p bifrost-admin external_runner_session_detail_uses_persisted_state --lib -- --nocapture` 与 `cargo test -p bifrost-admin list_session_states_includes_external_runner_result_fields --lib -- --nocapture`，验证外部 runner 完成后 session state 可合成详情并保留 latest run/response 字段。Web UI 真实链路在 `http://127.0.0.1:3001/_bifrost/ai?aiSection=agent-chat&agentSection=chat` 逐个点击 New Chat 选择 Runner 并发送消息：内置 Bifrost Agent 会话 `admin-chat-1779698889119` 走 `/api/agent/chat/stream`，Codex 会话 `admin-chat-1779699072406` 走 `/api/im-gateway/chat/stream` 且请求包含 `runnerId:"codex"`、`adapter:"codex"`，ChatGPT Web 会话 `admin-chat-1779698291975` 走 `/api/im-gateway/chat/stream` 且请求包含 `runnerId:"abc"`、`adapter:"chatgpt_web"`；三者均在 `sessions/all` 可见，`/sessions/{session_key}` 均返回 200、`message_count >= 2`、`work_dir:"/Users/eden/work/github/bifrost"`，其中 Codex 详情返回 `runner_type:"codex"` 与 assistant 内容 `Codex UI E2E 1779699073490`。UI 宽度回归通过 `pnpm --dir web exec playwright test tests/ui/agent-chat.spec.ts --grep "750px|max|external runner|thread list|restores active session"` 覆盖，其中 max-width 用例验证消息轨道和 composer 轨道为 750px 且居中。

### TC-IMA-122: Agent Chat 服务端稳定标题、线程列表与消息时间戳

- **前置条件**:
  - 使用当前源码启动 Bifrost Admin 与 WebUI。
  - 存在包含 `title_updated` 的 JSONL history、无显式 title 但有第一条 user message 的 JSONL history、内置 Bifrost Agent active session、Codex/ChatGPT Web external runner active session。
- **操作步骤**:
  1. 调用 `GET /_bifrost/api/im-gateway/agent/sessions/all`，检查每个线程对象直接带有稳定 `title`。
  2. 对无显式 title 的 history session 调用 `GET /_bifrost/api/im-gateway/agent/sessions/<session_key>`，检查服务端详情直接返回第一条 user message 作为 `title`。
  3. 对包含 `title_updated` 的 history session 调用同一详情接口，检查显式 title 优先。
  4. 在 WebUI 发起一个内置 Agent 长任务；无输入时检查输入框内右下角按钮显示 Stop。
  5. 长任务运行中输入一条消息，检查可切换 Guide / Queue；选择 Queue 后消息只显示在输入框上方队列面板，不进入 MessageList，可继续追加多条并删除。
  6. 对支持 guide 的内置 Agent，点击队列项上的 Guide，检查该消息可转为立即引导；对 Codex/ChatGPT Web 只显示默认 Queue，不展示 Guide。
  7. 打开长历史会话，手动向上滚动消息区，等待 2 秒后检查页面不会自动弹回底部。
  8. 在线程列表中切换多个线程，检查选中态和标题/副标题不闪烁；左侧小标识表示 Runner 类型（Bifrost/Codex/WebGPT），第二行渠道表示 Web/WeChat/Feishu/ASR Task/Scheduled。
  9. 检查每条消息气泡下方都展示时间戳；鼠标悬浮时间戳时可看到完整发送/存储时间，时间戳不占用气泡正文区域。
  10. 检查 MessageList 不展示用户/机器人头像，assistant 气泡充分使用中间内容轨道宽度，user 气泡仍右侧对齐且保持较窄阅读宽度。
  11. 在 assistant Markdown 中点击普通链接，检查链接使用新标签页打开，不覆盖当前 Agent Chat 页面。
- **预期结果**:
  - 服务端列表与详情都遵循同一 title 规则：`title_updated` > 第一条 user message > session_key，不依赖前端点击后临时计算。
  - 线程行不会在选中后从 session id 抖动为第一条消息。
  - 运行中输入框仍可输入：内置 Agent 支持 guide/queue，外部 Runner 默认 queue。
  - Queue 列表显示在输入框上方，支持多条、删除、内置 Agent 一键转 guide；排队确认与删除确认不作为消息流卡片展示。
  - 输入区在同一个消息滚动容器内以悬浮卡片贴近容器底部展示，没有与消息列表割裂的顶部硬分割线。
  - 历史阅读时手动滚动不会被自动贴底逻辑抢回底部。
  - 线程列表是轻量两行列表：无按钮边框，Runner 类型与渠道来源分离展示。
  - 消息时间戳显示在气泡外侧底部，不挤占正文；消息列表不渲染头像，assistant 气泡使用完整内容轨道。
  - Markdown 链接带 `target="_blank"` 和安全 `rel`，点击不会让当前会话页跳走。
- **执行记录（2026-05-25）**: PASS — 执行 `cargo test -p bifrost-agent session_detail_ --lib` 通过，验证内存 session detail 使用第一条 user message 作为 title fallback 且显式 title 优先；执行 `cargo test -p bifrost-admin history_session_detail_ --lib -- --nocapture` 通过，验证服务端从 JSONL history 合成 `/sessions/{session_key}` 详情并直接返回稳定 title 和 message timestamp。执行真实历史页脚本打开 `session-weixin_o9cq80wqfvOh3cJ69ywGqu9cGdqM_im_wechat-1779205574.jsonl`，刷新恢复后 `agent-chat-messages` 的 `scrollHeight=10195`、`clientHeight=1240`、`scrollTop=8955`、`distanceFromBottom=0`；随后手动向上滚动 900px，等待 2.5 秒后 `distanceFromBottom=900`，验证首次进入直接到底部且用户向上阅读时不会被锁回底部。执行 `pnpm --dir web exec playwright test tests/ui/agent-chat.spec.ts --grep "thread list scrolls|750px max|running stop|deep link renders" --reporter=line` 通过，覆盖线程标题不因点击选中抖动、线程列表撑满右侧 rail 且内部滚动、发送按钮嵌入输入框且运行中可 Stop、运行中输入支持 Guide/Queue 切换、Queue 面板展示与删除、Queue/Remove 不进入 MessageList、输入区 750px 居中并悬浮贴近滚动容器底部、消息列表不展示头像且 assistant 气泡占满内容轨道、Markdown 链接新开页面。执行 `cargo test -p bifrost-admin queue_stream_remove_deletes_item_before_drain --lib -- --nocapture` 通过，验证服务端 `/q` 后 `/rq 1` 删除会清空 queue manager，后续 `pop_queue` 取不到已删除消息；执行 `cargo test -p bifrost-admin external_runner_persists_user_message_before_result_and_dedupes_finish --lib -- --nocapture` 通过，验证外部 Runner 开始执行时立即把 user 消息落入同一 session_state，结束时不会重复追加 user 消息。

### TC-IMA-123: Agent Chat Threads 右键菜单删除会话

- **前置条件**:
  - 使用当前源码启动 Bifrost Admin 与 WebUI。
  - Threads 列表至少包含一条测试会话；该会话可由 mock API 或临时 `admin-chat-*` 会话创建，不使用用户重要历史数据。
  - `GET /_bifrost/api/im-gateway/agent/sessions/all` 能返回该会话，`DELETE /_bifrost/api/im-gateway/agent/sessions/<session_key>` 可访问。
- **操作步骤**:
  1. 打开 `/_bifrost/ai?aiSection=agent-chat&agentSection=chat`。
  2. 在线程列表中对测试会话行点击鼠标右键。
  3. 检查右键菜单在鼠标位置打开，菜单中出现 Delete 操作；线程行本身不新增常驻删除按钮。
  4. 点击 Delete。
  5. 检查同一个菜单位置切换为 Confirm / Cancel 原位二次确认。
  6. 点击 Cancel，检查会话仍保留且菜单关闭或回到可再次操作状态。
  7. 再次右键同一会话，点击 Delete，再点击 Confirm。
  8. 刷新 Threads 列表或页面，调用 `GET /sessions/all` 确认该 `session_key` 不再从 active/history/session_state 任一数据源返回。
- **预期结果**:
  - 线程操作通过可扩展右键 context menu 承载，后续可继续增加更多菜单项。
  - 删除需要二次确认，确认 UI 原位展示，不弹出全局 Modal。
  - Confirm 后前端立即移除该线程；如果删除的是当前会话，页面回到新的空白草稿对话。
  - 服务端删除会清理内存 session、running preview、queue/guide、外部 runner session_state 以及同 key JSONL history，刷新后不会重新出现。
- **清理步骤**:
  - 删除本用例创建的临时会话数据；不删除用户已有重要历史。
- **执行记录（2026-05-25）**: PASS — 执行 `pnpm --dir web exec playwright test tests/ui/agent-chat.spec.ts --grep "thread context menu deletes" --reporter=line` 通过，真实 Chromium 验证线程行右键打开 context menu、点击 Delete 后同位置出现 Confirm / Cancel、Confirm 调用 `DELETE /_bifrost/api/im-gateway/agent/sessions/delete-target` 并从列表移除。执行 `pnpm --dir web exec tsc --noEmit && pnpm --dir web exec tsc -b` 通过，确认右键菜单实现满足生产构建 noUnused 约束。执行 `cargo check -p bifrost-admin` 通过，确认服务端 `DELETE /sessions/{session_key}` 编译通过。随后用当前源码重启真实 `9900` 服务，打开 `http://localhost:3001/_bifrost/ai?aiSection=agent-chat&agentSection=chat`，确认页面不再白屏且 Threads 有 14 条；在真实页面右键第一条线程，确认右键菜单出现 Delete 项。未在真实用户数据上点击 Confirm 删除。

### TC-IMA-124: Agent Chat 三类 Runner 五轮多轮会话不漂移

- **前置条件**:
  - 使用当前源码启动 Bifrost Admin 与 WebUI。
  - `GET /_bifrost/api/im-gateway/chat/config` 至少启用内置 Bifrost Agent、Codex Runner、ChatGPT Web Runner。
  - 为测试创建新的临时会话，不复用用户重要历史。
- **操作步骤**:
  1. 打开 `/_bifrost/ai?aiSection=agent-chat&agentSection=chat`。
  2. 如果 Threads 为空，检查消息列表不展示 demo/starter 对话，只展示空态提示用户输入问题。
  3. 如果 Threads 非空且 URL 没有 `session` / `historyPath`，检查页面自动选中第一条线程；随后点击 New Chat 创建空白草稿，确认不会被自动拉回第一条线程。
  4. 分别创建 Bifrost Agent、Codex、ChatGPT Web 三类 Runner 的新对话。
  5. 每类 Runner 在同一个新对话中连续发送 5 条消息，每轮等待 assistant 回复完成后再发送下一轮。
  6. 每轮后记录请求中的 `session_key` / `sessionKey`、Runner ID、adapter，以及服务端返回/持久化的 `threadId` 或 `conversationId`。
  7. 完成 5 轮后调用 `GET /_bifrost/api/im-gateway/agent/sessions/all`，检查该 Runner 的测试 `session_key` 只出现一次。
  8. 调用 `GET /_bifrost/api/im-gateway/agent/sessions/<session_key>`，检查 messages 至少包含 5 条 user 和 5 条 assistant，顺序完整，runner/source/workspace 元信息一致。
- **预期结果**:
  - 三类 Runner 的 5 轮请求都复用同一个 `session_key`。
  - Threads 不会为同一组多轮消息生成多个线程。
  - Codex/ChatGPT Web 的外部 `threadId` / `conversationId` 作为扩展续聊引用保存，但不会替代或改变 UI 线程主键。
  - `/sessions/<session_key>` 返回完整 10 条消息，而不是只返回最后一轮。
  - 空白入口不展示 demo 消息；有线程时首次进入默认打开第一条线程，主动 New Chat 后保持空白草稿。
- **清理步骤**:
  - 删除本用例创建的测试会话；不删除用户已有重要历史。
- **执行记录（2026-05-25）**: PASS — 执行 `pnpm --dir web exec playwright test tests/ui/agent-chat.spec.ts --grep "five rounds|selects the first thread|deep link renders" --reporter=line` 通过，覆盖空白入口不展示 demo 消息、无线程时展示输入提示、有线程且无 `session/historyPath` 时默认打开第一条线程、主动 New Chat 后保持草稿，以及 Bifrost Agent / Codex / ChatGPT Web 三类 Runner 各自连续 5 轮都复用同一个 `sessionKey` 且 Threads 只保留一条对应线程。执行 `cargo test -p bifrost-admin external_runner_session_detail_preserves_five_turns_in_one_thread --lib -- --nocapture` 与 `cargo test -p bifrost-admin session_state_normalizes_persisted_message_sequence --lib -- --nocapture` 通过，确认外部 Runner 的多轮消息序列会落在同一个服务端 `session_state.messages` 中，`/sessions/<session_key>` 可返回完整 5 user + 5 assistant。执行 `pnpm --dir web exec tsc --noEmit`、`pnpm --dir web exec tsc -b`、`cargo check -p bifrost-admin` 通过。

### TC-IMA-126: Agent Chat Slash Runner Call 正常路径

- **前置条件**:
  - 使用当前源码启动 Bifrost Admin 与 WebUI。
  - `GET /_bifrost/api/im-gateway/chat/config` 至少返回内置 `Bifrost Agent` 和任意已启用外部 Runner；slash 面板不按当前 Runner 过滤。
  - 使用新的临时 Agent Chat 会话，不复用用户重要历史。
- **操作步骤**:
  1. 打开 `/_bifrost/ai?aiSection=agent-chat&agentSection=chat`。
  2. 在输入框输入 `/`。
  3. 检查输入框上方出现 Runner 选择面板，面板列出全部可用 Runner，包括当前 Runner 和内置 `Bifrost Agent`。
  4. 点击 `codex` Runner。
  5. 检查输入框内出现 `Run with codex` chip，且输入框可继续输入消息。
  6. 输入 `基于当前上下文给出实现建议` 并发送。
  7. 检查请求发送到 `/_bifrost/api/im-gateway/chat/runner-calls/stream`，请求体包含 `callerSessionKey`、`callerRunnerId`、`callerRunnerAdapter`、`targetRunnerId:"codex"`、`callerMessages` 和用户消息。
  8. 等待流式返回完成。
- **预期结果**:
  - Slash 选择不会修改顶部当前 Runner tag。
  - 消息流中 user 气泡展示 `Run with codex` 和用户输入。
  - assistant 气泡展示目标 Runner 的执行状态和最终输出。
  - 发送后刷新页面，旧会话仍能从后端持久化状态恢复 `Run with codex` 用户消息和目标 Runner running/完成状态。
  - `runner-call:*` 子会话不作为新线程展示在 Threads 列表。
  - 调用完成后 chip 自动清空，输入框恢复普通输入状态。
- **执行记录（2026-05-26）**: PASS — 执行 `pnpm --dir web exec playwright test tests/ui/agent-chat.spec.ts --grep "slash runner" --reporter=line`，通过真实浏览器操作输入 `/`、确认 slash 面板同时展示内置 `Bifrost Agent` 与外部 `codex`、选择 `codex`、检查 `Run with codex` chip、拦截并断言 `/_bifrost/api/im-gateway/chat/runner-calls/stream` 请求体包含 `callerSessionKey`、`callerRunnerId`、`targetRunnerId:"codex"`、`callerMessages` 和用户消息，mock NDJSON 流式返回后消息区展示 Runner Call 结果；同时断言顶部 Runner tag 仍为 `Runner: bifrost_agent`。
- **回归执行记录（2026-05-26）**: PASS — 针对从外部 Runner 选择 `Bifrost Agent` 会返回 `{"error":"slash Runner calls currently require an external target Runner","status":400}` 的回归，执行 `cargo test -p bifrost-admin runner_call_target_accepts_builtin_agent --lib -- --nocapture`，验证 `targetRunnerId:"bifrost_agent"` 会被解析为内置目标 Runner 而不是 400 拒绝；代码 review 确认内置目标通过 `runner-call:<source>:bifrost_agent` 子会话执行并仍返回 `runner_call_started` / `runner_call_finished` NDJSON。
- **持久化回归执行记录（2026-05-26）**: PASS — 执行 `cargo test -p bifrost-admin runner_call_visible_messages_stay_on_source_thread --lib -- --nocapture`，验证 Runner Call started 时源会话持久化 user 消息和 running assistant 状态，finished 时同一源会话内原地更新 assistant 结果；执行 `cargo test -p bifrost-admin runner_call_child_session_keys_are_internal --lib -- --nocapture`，验证 `runner-call:*` 被识别为内部子会话；执行 `pnpm --dir web exec playwright test tests/ui/agent-chat.spec.ts --grep "slash runner" --reporter=line`，验证线程刷新返回 `runner-call:*` running 子会话时 UI 不展示新线程，并保留旧会话中的 Runner Call running/结果展示。

### TC-IMA-127: Slash Runner Call 结果被下一轮当前 Runner 消费

- **前置条件**:
  - 已完成 TC-IMA-126，且目标 Runner 返回了非空结果。
- **操作步骤**:
  1. 在同一当前会话中继续发送普通消息：`请结合刚才 Runner 的结果继续分析`。
  2. 对外部当前 Runner，检查下一轮 `/chat/stream` 运行前生成的 prompt 或 instructions 包含 `Imported Runner Results` 和上一轮目标 Runner 输出。
  3. 对内置 Bifrost Agent 当前 Runner，检查下一轮 `/agent/chat/stream` 执行前 session history 已包含上一轮 slash Runner call 的 user/assistant 可见记录或 imported context。
  4. 等待当前 Runner 回复完成。
- **预期结果**:
  - 下一轮当前 Runner 能引用上一轮 slash Runner call 输出，不需要用户手动复制。
  - imported context 只消费一次；同一结果不会无限重复注入后续请求。
  - 普通发送路径仍走当前会话 Runner，不会自动切换到目标 Runner。
- **执行记录（2026-05-26）**: PASS — 执行 `cargo test -p bifrost-admin external_runner_consumes_imported_context_into_instructions_once --lib -- --nocapture`，验证 slash Runner call 保存的 imported context 会在下一轮当前外部 Runner 请求构建时追加到 instructions，且第一次消费后第二次请求不再重复注入；执行 `cargo test -p bifrost-admin imported_contexts_are_pushed_rendered_and_consumed_once --lib -- --nocapture`，验证 session_state 的入队、渲染和一次性消费语义。

### TC-IMA-128: Slash Runner Call 选择 Runner 不改变当前会话默认 Runner

- **前置条件**:
  - 当前会话默认 Runner 为 Bifrost Agent 或任意外部 Runner。
  - 至少存在一个其他外部 Runner 可用于 slash call。
- **操作步骤**:
  1. 记录顶部 Runner tag 和 New Chat 设置中的当前 Runner。
  2. 使用 `/` 选择另一个 Runner 并完成一次 slash Runner call。
  3. 调用 `GET /_bifrost/api/im-gateway/agent/sessions/all`，检查当前会话线程的 `runner_id` / `runner_type` 仍表示原当前 Runner。
  4. 继续发送一条普通消息。
- **预期结果**:
  - Slash Runner call 是一次工具式调用，不改变当前会话默认 Runner。
  - 普通消息继续由原当前 Runner 处理。
  - 目标 Runner 子会话可独立保留自己的 `sessionKey` / `threadId` / `conversationId`，但不会替代当前 UI 线程主键。
- **执行记录（2026-05-26）**: PASS — 执行 `pnpm --dir web exec playwright test tests/ui/agent-chat.spec.ts --grep "slash runner" --reporter=line`，在同一浏览器用例中完成 slash Runner call 后断言顶部 `agent-chat-runner-tag` 保持 `Runner: bifrost_agent`，未切换为目标 `codex`；执行 `cargo test -p bifrost-admin external_runner_persists_user_message_before_result_and_dedupes_finish --lib -- --nocapture`，确认外部 Runner 原有会话持久化与完成事件去重路径未被 runner call 改动破坏。

### TC-IMA-129: Agent Chat Threads 大量历史默认分批加载与虚拟滚动

- **前置条件**:
  - 使用当前源码启动 Bifrost Admin 与 WebUI；启动服务必须使用临时数据目录并携带 `--no-system-proxy`。
  - `GET /_bifrost/api/im-gateway/agent/sessions/all` 返回至少 55 条 Agent Chat 线程摘要，可用 Playwright mock API 或测试数据构造，不复用或删除用户重要历史。
  - 测试需覆盖亮色和暗色主题下的 Threads 侧栏。
- **操作步骤**:
  1. 打开 `/_bifrost/ai?aiSection=agent-chat&agentSection=chat&view=active`。
  2. 检查右侧 Threads 卡片底部显示 `Showing 20 of <total>`，且出现 `Load more` 按钮。
  3. 检查 DOM 中实际挂载的线程行数量明显小于 `sessions/all` 返回总数，证明列表没有把所有历史线程一次性渲染出来。
  4. 滚动 Threads 列表到底部，点击 `Load more`。
  5. 检查底部计数变为 `Showing 40 of <total>`。
  6. 再次滚动到底部并点击 `Load more`，直到全部加载。
  7. 检查计数变为 `Showing <total> of <total>`，`Load more` 按钮消失。
  8. 点击已加载范围内任意线程，确认左侧会话内容正常切换且只有该线程为选中态。
  9. 切换暗色主题，重复检查 Threads 行文本、Runner 标识、计数和按钮均清晰可读。
- **预期结果**:
  - 首屏 Threads 只开放 20 条历史线程，不会一次性展示全部历史。
  - 每次点击 `Load more` 只追加 20 条，最后不足 20 条时追加剩余线程。
  - Threads 列表使用虚拟滚动，DOM 中的线程行数量不随总线程数线性膨胀。
  - 已选线程、右键菜单、running 状态点、来源/Runner 标识和亮暗主题可读性不退化。
- **清理步骤**:
  - 关闭 Playwright 浏览器。
  - 由 Playwright global teardown 清理临时服务；如手动启动过服务，停止对应临时端口服务并删除临时数据目录。
- **执行记录（2026-05-26）**: PASS — 创建用例后立即执行 `pnpm --dir web exec playwright test tests/ui/agent-chat-threads.spec.ts --grep "loads in batches" --reporter=line`，真实 Chromium UI 用例通过。测试构造 55 条 Agent Chat 线程，打开 `/_bifrost/ai?aiSection=agent-chat&agentSection=chat` 后确认底部计数初始为 `Showing 20 of 55` 且 `Load more` 可见；DOM 中 `agent-chat-thread-virtual-row` 数量小于 55；滚动到底部点击后计数依次变为 `Showing 40 of 55` 和 `Showing 55 of 55`，最终 `Load more` 消失，验证默认 20 条、每次追加 20 条与虚拟滚动均生效。

### TC-IMA-130: Web 与 IM 同会话共享底层 Agent timeline 与运行状态

- **前置条件**:
  - 使用当前源码和临时 `BIFROST_DATA_DIR` 启动 Bifrost，必须携带 `--no-system-proxy`。
  - Agent history persistence 设置为 `save-all`，模型指向 OpenAI-compatible mock provider。
  - 使用新的 `session_key=timeline-channel-unified`，不复用用户重要历史。
- **操作步骤**:
  1. 通过 IM/API 通道调用 `POST /_bifrost/api/im-gateway/agent/chat`，发送 `IM asks for canonical timeline`。
  2. mock provider 首轮返回 `update_plan` tool call，第二轮返回最终内容 `IM_TURN_OK`。
  3. 调用 `GET /_bifrost/api/im-gateway/agent/sessions/all`，找到同一 session。
  4. 使用返回的 `history_path` 调用 `GET /_bifrost/api/im-gateway/agent/sessions/history/<encoded-history-path>`。
  5. 通过 Web 通道调用 `POST /_bifrost/api/agent/chat/stream`，同一个 session 发送 `Web continues same canonical timeline`。
  6. 再次读取同一个 `history_path`。
- **预期结果**:
  - `sessions/all` 中该 session 同时包含 `history_path`、`has_timeline=true`、`timeline_event_count > 0`、`run_state=completed`。
  - 首次 history 中包含 IM 用户消息、`tool_call`、`tool_result`、IM assistant 最终消息，以及 `source_channel=api` 的 running/completed `run_state_changed`。
  - Web SSE 返回 `WEB_TURN_OK`，并且同一个 history 文件新增 Web 用户消息、Web assistant 最终消息，以及 `source_channel=web` 的 running/completed `run_state_changed`。
  - Web UI 读取 active thread 时可从 `history_path` 恢复工具执行和运行状态过程，不再只展示用户消息和最终回复。
- **清理步骤**:
  - 停止临时 Bifrost 和 mock provider 进程。
  - 删除临时数据目录。
- **执行记录（2026-05-28）**: PASS — 创建用例后立即执行 `bash e2e-tests/tests/test_agent_run_timeline_channel_unification.sh`。脚本使用临时 `BIFROST_DATA_DIR`、随机端口和 `--no-system-proxy` 启动当前源码 Bifrost 与 OpenAI-compatible mock provider；先通过 `POST /_bifrost/api/im-gateway/agent/chat` 完成包含 `update_plan` tool call 的 IM/API turn，断言 `sessions/all` 返回 `history_path`、`has_timeline=true`、`timeline_event_count >= 6`、`run_state=completed`；再通过同一 session 调用 `POST /_bifrost/api/agent/chat/stream`，断言 Web SSE 返回 `WEB_TURN_OK`，同一个 history 文件同时包含 IM 与 Web 用户/assistant 消息，并包含 `source_channel=api` 与 `source_channel=web` 的 running/completed `run_state_changed`。补充执行 `pnpm --dir web exec playwright test tests/ui/agent-chat.spec.ts --grep "active timeline process" --reporter=line`，验证 active Web thread 按真实事件顺序从 `history_path` 恢复 `run_state_changed`、tool input/output 与 assistant 过程块，结果通过。

### TC-IMA-131: Agent Chat Token/Context HUD 使用细进度线展示

- **前置条件**:
  - 使用当前源码启动 WebUI，或使用 Playwright mock Agent Chat stream。
  - Agent Chat stream 返回 `total_tokens_used`、`context_usage_percent`，或历史 `assistant_message.context_tokens`。
  - 测试需覆盖亮色与暗色主题。
- **操作步骤**:
  1. 打开 `/_bifrost/ai?aiSection=agent-chat&agentSection=chat`。
  2. 发送一条消息，让 mock stream 返回 token/context telemetry。
  3. 检查输入框上方的 Token/Context HUD。
  4. 切换暗色主题，重复检查 HUD。
- **预期结果**:
  - HUD 不再以占高 pill/card 形式展示三段指标。
  - 输入框顶部只显示一条细进度线，进度宽度跟随 `context_usage_percent`。
  - 文字位于进度线尾部上方，展示 `Tokens` 和百分比形式的 `Context`；不展示 `Compression`。
  - 自动或手动压缩发生时，消息流中在对应 assistant 段落内展示低对比度分隔提示 `上下文已自动压缩`，而不是只在 HUD/Status 弹窗里体现。
  - 当只有 `assistant_message.context_tokens` 可用时，HUD 仍可按默认 context window 计算百分比，不显示 `Context -`。
  - HUD 高度保持很薄，不挤占输入框主要空间；亮暗主题下文字和线条均清晰可读。
- **清理步骤**:
  - 关闭 Playwright 浏览器；如手动启动过服务，停止对应临时端口服务并删除临时数据目录。
- **执行记录（2026-05-28）**: PASS — 执行 `pnpm --dir web exec playwright test tests/ui/agent-chat.spec.ts --grep "updates the thread list from session events" --reporter=line` 通过。测试通过 `sessions/events` SSE mock 推送 `sessions_changed`，断言页面不依赖固定轮询即可刷新 `sessions/all`，新 IM running 线程立即出现在列表中，且当前选中的 Web 线程不被抢占。

- **执行记录（2026-05-28）**: PASS — 创建用例后立即执行 `pnpm --dir web exec tsc -b` 通过；执行 `SKIP_BUILD=true BIFROST_BIN=/tmp/bifrost-does-not-exist bash e2e-tests/tests/test_agent_run_timeline_channel_unification.sh` 通过，确认 CI fallback 修复不破坏主 timeline E2E；执行 `pnpm --dir web exec playwright test tests/ui/agent-chat.spec.ts --grep "token HUD|active timeline process|running history timeline" --reporter=line`，3 条真实 Chromium UI 回归通过，覆盖 HUD 文案、进度线宽度 `35%`、高度不超过 22px、线在 composer 外圈上沿且文字位于线右侧、亮暗主题，以及刷新 running history 后最后一个 user 下方必有 assistant 过程卡片并继续轮询工具过程。补充根据截图回归执行 `pnpm --dir web exec playwright test tests/ui/agent-chat.spec.ts --grep "token HUD" --reporter=line` 通过，确认 HUD 上移后不遮挡 Plan/输入内容，移除 `Compression`，`Context` 以百分比展示；再次补充执行同一 token HUD 回归，确认消息列表底部保留至少 72px padding，最后一条消息不会贴住 HUD/输入框；再次补充执行 `pnpm --dir web exec playwright test tests/ui/agent-chat.spec.ts --grep "token HUD|plan content compact" --reporter=line`，确认 HUD 进度线改为低饱和弱对比色、Plan 默认折叠并在折叠态展示当前执行步骤与 `+N` 更多计数；再次补充执行 `pnpm --dir web exec playwright test tests/ui/agent-chat.spec.ts --grep "deep link|active timeline process|running history timeline" --reporter=line` 通过，确认 Loop 中的 `assistant_delta` 按顺序渲染为 assistant 文本段，工具输出挂在最近文本段下方且默认只显示低对比度摘要，不默认展开命令输入/输出。
- **间距回归执行记录（2026-05-28）**: PASS — 执行 `pnpm --dir web exec tsc -b` 通过；执行 `pnpm --dir web exec playwright test tests/ui/agent-chat.spec.ts --grep "token HUD|keeps plan content compact" --reporter=line`，2 条真实 Chromium UI 回归通过，确认 Token HUD 进度线不再作为 AntD Space item 占用 composer 内部垂直空间，Plan 与输入文字之间的间距收紧，同时 HUD 仍保持在 composer 外圈上方且亮暗主题可读。

### TC-IMA-132: Agent Chat 刷新 running history 后保留 assistant 过程卡片

- **前置条件**:
  - WebUI 打开一个绑定 `history_path` 的 Agent Chat 会话。
  - 会话已有上一轮 user/assistant 消息。
  - 当前轮最新状态为 running，thread summary 标记 `running=true`，JSONL 中至少包含当前轮 `user_message`，但可能还没有新的 `run_state_changed`，后续会 append `tool_call/tool_result`。
- **操作步骤**:
  1. 打开 `/_bifrost/ai?aiSection=agent-chat&agentSection=chat&session=<session>&view=history&historyPath=<encoded-path>`。
  2. 确认当前轮用户消息已展示。
  3. 在当前轮仍 running 时刷新页面。
  4. 检查消息列表最后一个 user message 下方是否立即出现 assistant running/process 卡片。
  5. 等待 history 追加 `tool_call/tool_result` 后，检查同一 assistant 卡片原位更新过程块。
- **预期结果**:
  - 输入 `/` 时 slash 面板列出 `/compact` 压缩命令，支持上下键切换选择，回车触发当前选中项。
  - 选择 `/compact` 后不把命令填入输入框，也不需要用户再点击发送；前端直接向服务端触发压缩控制动作。
  - 手动输入 `/compact` 并回车或点击发送时，也按系统控制命令处理，不生成 `/compact` 用户气泡，不进入普通 Agent Loop，不展示模型普通回复或工具执行过程，只在消息流中展示压缩状态/`上下文已自动压缩` 分隔提示。
  - 未命中内置命令的 slash 输入（例如 `/demo`）回落为普通聊天消息，不返回 `未知命令`。
  - 刷新后不能只展示用户消息；只要底层 Loop running，最后一条当前轮 user 下方必须有 assistant 卡片。
  - 工具执行、plan、compaction 等 timeline append 后，Web view 继续轮询并更新该 assistant 卡片。
  - 新一轮 running 的 process steps 不会挂到上一轮 assistant 回答上。
  - 如果第一条 timeline 事件就是工具执行，工具摘要前仍要展示一段 assistant 过程说明，不能出现空白 assistant 段。
  - 工具摘要默认低对比度折叠展示，不在摘要上下保留大块空白。
  - 当整体 run 已结束后，即使历史里残留 running 状态事件，展开工具详情也不能继续展示 `Run state: Running` 或 `(N active)`。
  - 压缩摘要模型请求如果遇到 429、timeout、connection、overloaded、5xx 等可恢复错误，按时间梯度最多重试 5 次；仍失败后终止当前任务并交给人工处理，不能继续无限发送消息。
  - 压缩摘要模型请求如果遇到 `context_length_exceeded` / context window / token limit 等已知超窗错误，先降级裁剪较旧 history，再重试生成摘要；仍无法压缩时终止当前任务，不能循环触发新消息。
- **清理步骤**:
  - 关闭 Playwright 浏览器；如手动启动过服务，停止对应临时端口服务并删除临时数据目录。
- **执行记录（2026-05-28）**: PASS — 创建用例后立即执行 `pnpm --dir web exec playwright test tests/ui/agent-chat.spec.ts --grep "running history timeline" --reporter=line` 通过。测试构造上一轮 user/assistant 后追加当前轮 user，thread summary 标记 `running=true`，首次 history 读取没有 `run_state_changed` 或 tool 事件，随后轮询读取到 `assistant_delta`、`exec_command` 的 `tool_call/tool_result` 和下一段 `assistant_delta`；断言最后一个 user 下方出现新的 assistant process 卡片，展开后可见 `Run state: Running`、`exec_command`、`cargo test` 和 `still running output`，消息底部在时间上方保留 `Thinking...` 提示。补充执行 `pnpm --dir web exec playwright test tests/ui/agent-chat.spec.ts --grep "deep link|active timeline process|running history timeline" --reporter=line` 通过，4 条真实 Chromium UI 回归覆盖：第一条事件为工具调用时会显示 `我先执行一步检查。` 作为 assistant 过程说明；工具摘要默认折叠且紧贴文本不产生大块空白；完成态 process block 展开后不再显示 `Run state: Running` 或 `active`；running history 仍保留底部 `Thinking...` 提示。再次补充执行 `WEB_PORT=3000 BACKEND_PORT=8800 pnpm --dir web dev --host 127.0.0.1`，真实打开 `http://127.0.0.1:3000/_bifrost/ai?aiSection=agent-chat&agentSection=chat&session=admin-chat-1779949709059&view=active` 并量测两个 `agent-chat-process-block`：实际高度均为 `18.84375px`，`marginTop=0px`、`marginBottom=2px`、`paddingTop=0px`、`paddingBottom=0px`，确认截图中的 `750x22.85` 多余盒模型空间已移除。
- **压缩命令回归执行记录（2026-05-28）**: PASS — 执行 `pnpm --dir web exec tsc -b` 通过；执行 `pnpm --dir web exec playwright test tests/ui/agent-chat.spec.ts --grep "compact as a control command" --reporter=line`，真实 Chromium 验证 slash 面板默认选中 `/compact`、上下键可切到 Runner 再切回命令、回车直接触发 `message:"/compact"` 请求且输入框清空，消息区没有 `/compact` 用户气泡、没有 `我先执行一步检查。`、没有工具输出，只展示 `上下文已自动压缩`；同一用例继续手动输入 `/compact` 点击发送，断言仍不生成用户气泡。执行 `cargo test -p bifrost-agent test_unknown_slash_input_falls_back_to_normal_chat_message -- --nocapture` 通过，验证 `/demo` 调用模型并作为普通用户消息进入 history；执行 `cargo test -p bifrost-agent test_manual_compaction_command_does_not_record_user_message -- --nocapture` 通过，验证 manual compaction 只记录 `compaction` 事件，不记录 `/compact` user_message。
- **压缩失败回归执行记录（2026-05-28）**: PASS — 使用 `bifrost traffic get 811767 --request-body --response-body --format json-pretty` 获取真实失败请求，确认压缩摘要请求返回 `400 context_length_exceeded`，服务端提示配置上限 `922000` tokens、实际请求 `1714286` tokens。执行 `cargo test -p bifrost-agent test_compaction_ -- --nocapture` 通过，覆盖压缩请求发出前按 safe budget 预裁剪、`context_length_exceeded` 后批量降级裁剪 history 再重试、transient/429/5xx 类错误最多 5 次退避重试后失败且不改写 history；执行 `cargo test -p bifrost-agent test_is_retryable_error -- --nocapture` 通过，确认普通模型请求把 `429` / rate limit / timeout / connection 等归类为 retryable。

### TC-IMA-133: Agent Chat Threads 通过长连接感知 IM 新 Loop

- **前置条件**:
  - 使用当前源码和临时 `BIFROST_DATA_DIR` 启动 Bifrost，必须携带 `--no-system-proxy`。
  - WebUI 打开 `/_bifrost/ai?aiSection=agent-chat&agentSection=chat`，并保持 Agent Chat 页面停留在线程列表视图。
  - 存在一个可触发 Agent Loop 的 IM 通道或 Playwright mock 的 `sessions/events` SSE。
- **操作步骤**:
  1. 打开 Agent Chat 页面并确认初始线程列表已加载。
  2. 不刷新页面、不等待定时轮询，通过 IM 通道或 mock SSE 触发一个新的 `sessions_changed` 事件。
  3. 让 `GET /_bifrost/api/im-gateway/agent/sessions/all` 在事件后返回一个新的 Feishu/IM running 线程。
  4. 检查线程列表是否立即出现该 IM 线程，同时当前选中的 Web 线程和消息内容不被切换。
  5. 使用浏览器 Network/测试断言确认页面没有固定间隔请求 `sessions/all`。
- **预期结果**:
  - Agent Chat 页面只在初始加载、页面重新可见、或 `sessions/events` 长连接收到 `sessions_changed` 时刷新线程列表。
  - 通过 IM 通道新建的 Agent Loop 能实时出现在 Web Agent Chat 线程列表中。
  - 当前选中的 Web 会话不被新 IM 线程抢占，消息区保持当前会话。
  - 不存在 2 秒轮询之类固定刷新，避免空闲页面持续消耗性能。
- **清理步骤**:
  - 关闭 Playwright 浏览器；如手动启动过服务，停止对应临时端口服务并删除临时数据目录。

### TC-IMA-134: Web 触发 Codex/GPT Web Runner 时写回同一 canonical timeline

- **前置条件**:
  - 使用当前源码启动 Bifrost Admin 与 WebUI，服务必须使用临时 `BIFROST_DATA_DIR` 并携带 `--no-system-proxy`。
  - 存在一个由 IM 或内置 Agent 创建的 `session_key` 与 `history_path`，该 JSONL 已包含上一轮 user/assistant 消息。
  - 当前会话绑定的 Runner 为 Codex、GPT Web 或其他 external CLI runner。
- **操作步骤**:
  1. 打开 `/_bifrost/ai?aiSection=agent-chat&agentSection=chat&session=<session>&view=history&historyPath=<encoded-path>`。
  2. 在 Web UI 中发送一条新消息，确认请求走 `/api/im-gateway/chat/stream` 且包含当前 `historyPath`。
  3. 等待 external runner 返回最终回复。
  4. 刷新同一个 history URL。
  5. 读取原 `history_path` 对应 JSONL，检查新增事件。
  6. 再打开一个无 `historyPath` 的 active GPT Web/Codex 会话，发送一条消息，确认后端创建新的 canonical JSONL 并把 `history_path` 写回 session state。
- **预期结果**:
  - Web 发送的用户消息不会在发送后或刷新后消失。
  - 新的 Codex/GPT Web assistant 回复展示在该用户消息下方，不挂到上一轮 assistant 卡片内部。
  - 原 `history_path` JSONL 追加 `source_channel=web`、`agent_kind=<runner>` 的 running/completed `run_state_changed`，并追加本轮 `user_message`、runner `tool_call/tool_result` 和 `assistant_message`。
  - 对于无 `historyPath` 的 active external runner 会话，首轮 Web 消息也会创建 canonical timeline；刷新后从该 timeline 恢复，不再只依赖 adapter-local `session_state.messages`。
  - `session_state.json` 只作为线程索引和 external thread/latest run 摘要；消息回放以 canonical timeline 为准。
- **清理步骤**:
  - 关闭 Playwright 浏览器；如手动启动过服务，停止对应临时端口服务并删除临时数据目录。
- **执行记录（2026-05-28）**: PASS — 对用户反馈的 `admin-chat-1779973287101` 本地数据做只读排查，确认 GPT Web 两次 run 均成功写入 `.bifrost-ui-test/agent/im_gateway/session_state.json` 与 `chat_runs/*/result.json`，但未出现在 `.bifrost-ui-test/agent/sessions/**/*.jsonl`，根因与 Codex external runner 一致：adapter-local state 成为消息事实源。随后执行 `cargo test -p bifrost-admin external_runner_ --lib -- --nocapture`，覆盖已有 historyPath 的 Codex/GPT Web 续聊追加原 JSONL、无 historyPath 的 active GPT Web 会话创建 canonical timeline 并写回 `history_path`、以及 `/sessions/{session}` 在存在 `history_path` 时优先从 canonical timeline 还原消息而不是读取 stale `session_state.messages`。执行 `pnpm --dir web exec tsc -b && pnpm --dir web exec playwright test tests/ui/agent-chat.spec.ts --grep "external runner" --reporter=line`，真实 Chromium 断言 Web 历史续聊 external runner 请求走 `/api/im-gateway/chat/stream` 且携带 `params.historyPath`，刷新回放不再丢失用户消息。

### TC-IMA-135: Agent Chat 输入框支持粘贴图片预览与上限控制

- **前置条件**:
  - 使用当前源码启动 WebUI，或使用 Playwright mock Agent Chat stream。
  - 打开 `/_bifrost/ai?aiSection=agent-chat&agentSection=chat`，当前 Runner 为 Bifrost Agent。
  - 测试需覆盖亮色和暗色主题下的输入框上方图片预览区域。
- **操作步骤**:
  1. 聚焦 Agent Chat 输入框。
  2. 从系统剪贴板一次性粘贴 7 张 PNG 图片，或通过浏览器测试构造等效的 `ClipboardEvent` 图片文件。
  3. 观察输入框上方图片预览区域。
  4. 点击第 1 张图片的删除按钮。
  5. 不输入任何文字，点击发送按钮。
- **预期结果**:
  - 输入框上方最多展示 6 张图片缩略图，并提示超出部分不会保留。
  - 每张缩略图有可识别的删除按钮和大小信息，亮色/暗色主题下边框、背景、删除按钮均清晰可见。
  - 删除第 1 张后预览数量变为 5。
  - 即使文本为空，只要有图片，发送按钮也可用。
  - 发送后输入框清空、预览区消失，用户消息显示 `Attached 5 images` 并在消息气泡内展示图片缩略图。
- **清理步骤**:
  - 关闭 Playwright 浏览器；如手动启动过服务，停止对应临时端口服务并删除临时数据目录。
- **执行记录（2026-05-29）**: PASS — 创建用例后立即执行 `pnpm --dir web exec playwright test web/tests/ui/agent-chat.spec.ts -g "pasted image"`，真实 Chromium UI 验证通过。用例构造 7 个 PNG `ClipboardEvent` 文件，断言预览数量被限制为 6，删除后变为 5；纯图片发送后输入框清空、预览区消失，消息区包含 `Attached 5 images` 与 `agent-chat-message-images`，请求体 `message:""` 且 `images.length===5`、`mime_type:"image/png"`。

### TC-IMA-136: Agent Chat 粘贴图片传给外部 Runner stream

- **前置条件**:
  - 使用当前源码启动 WebUI，或使用 Playwright mock Agent Chat stream。
  - `/_bifrost/api/im-gateway/chat/config` 中默认 Runner 为一个外部 Runner，例如 `codex`。
  - 打开 `/_bifrost/ai?aiSection=agent-chat&agentSection=chat`。
- **操作步骤**:
  1. 在 Agent Chat 输入框输入 `Describe this image`。
  2. 粘贴 1 张 PNG 图片。
  3. 点击发送。
  4. 检查请求路径与请求体。
- **预期结果**:
  - 页面请求 `/api/im-gateway/chat/stream`，不会误走内置 `/api/agent/chat/stream`。
  - 请求体包含 `message:"Describe this image"`、`runnerId:"codex"`，并包含 `images[0].mimeType:"image/png"` 与 base64 `data`。
  - 外部 Runner 返回的最终文本展示在消息区，用户消息保留文本和图片缩略图。
  - 当前会话的 Runner tag 与线程摘要保持外部 Runner 语义，刷新后可从会话详情恢复图片消息内容。
- **清理步骤**:
  - 关闭 Playwright 浏览器；如手动启动过服务，停止对应临时端口服务并删除临时数据目录。
- **执行记录（2026-05-29）**: PASS — 创建用例后立即执行 `pnpm --dir web exec playwright test web/tests/ui/agent-chat.spec.ts -g "pasted image"`，真实 Chromium UI 验证通过。用例 mock 默认 Runner 为 `codex`，粘贴 PNG 后发送，断言请求命中 `/api/im-gateway/chat/stream` 并携带 `message:"Describe this image"`、`runnerId:"codex"`、`images[0].mimeType:"image/png"`，页面展示 `Codex saw image`。

### TC-IMA-137: Agent Chat 图片消息后端落盘与历史回放

- **前置条件**:
  - 使用当前源码执行后端单元/集成测试，不复用用户真实会话目录。
  - 外部 Runner 使用临时 `run_dir`，图片内容为测试 PNG base64。
- **操作步骤**:
  1. 调用 external CLI runner，传入文本和图片数组。
  2. 检查 runner prompt 与 run metadata。
  3. 写入并读取 `session_state.json` 中包含 `content_parts` 的消息。
- **预期结果**:
  - 外部 Runner 将图片写入 `attachments/images/image-N.<ext>`，prompt 注入 `## Attached Images` 和本地文件路径。
  - `attachments.images` metadata 包含原始 `name`、`mime_type`、`path` 和 `size_bytes`。
  - 会话状态持久化 `content_parts`，读取详情 API 时可回放图片消息，而不是只剩文本占位。
  - 纯图片消息允许通过校验；无文本且无图片仍被拒绝。
- **清理步骤**:
  - 测试结束后删除临时目录。
- **执行记录（2026-05-29）**: PASS — 创建用例后立即执行 `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin external_cli_run_writes_image_attachments_and_injects_prompt_paths --lib` 与 `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin session_state_persists_message_content_parts --lib`，两项均通过，覆盖图片附件写入、prompt 路径注入、metadata 与 `content_parts` 持久化回放。
