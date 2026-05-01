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
    -d '{"default_system_prompt": "你是一个测试助手", "max_history_messages": 10}'
  ```
- **预期结果**: 返回 `{"success": true}`
- **验证步骤**: 再次 GET /agent 确认 default_system_prompt 和 max_history_messages 已更新

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

### TC-IMA-19: MCP 配置从 TOML 加载验证

- **前置条件**: `~/.bifrost-agent/config.toml` 中配置了 `[mcp_servers.lark]` 段
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

- **前置条件**: `work_dir` 配置为 `/Users/eden/work/github/bifrost`，项目目录中存在 `.agents/skills/` 子目录
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

### TC-IMA-25: Agent Tab 渲染 - 页面加载及配置展示

- **操作步骤**:
  1. 在浏览器中访问 `http://127.0.0.1:8800/_bifrost/settings?tab=agent`
  2. 检查 Settings 页面 Tab 列表中是否包含 "Agent" Tab（带 Robot 图标）
  3. 点击 Agent Tab，检查页面内容
- **预期结果**:
  - Settings Tab 列表显示 "Agent" Tab（带 Robot 图标）
  - 页面标题显示 "Agent Configuration"，状态标签显示 "Enabled"/"Disabled"
  - 显示 5 个折叠区域：General、Model Configuration、Runtime Settings、MCP Servers、Active Sessions
  - General 和 Model Configuration 默认展开
  - Model 字段显示 `gpt-5.4-2026-03-05`
  - Model Provider 字段显示 `aidp_crawl`

### TC-IMA-26: Agent Tab 配置修改 - PATCH API 即时生效

- **操作步骤**:
  ```bash
  # 修改 shell_timeout_secs 为 90
  curl -s -X PATCH http://127.0.0.1:8800/_bifrost/api/im-gateway/agent \
    -H 'Content-Type: application/json' \
    -d '{"shell_timeout_secs": 90}'
  ```
- **预期结果**:
  - API 返回完整的 AgentConfig JSON（不是 `{"success": true}`）
  - 返回的 JSON 中 `shell_timeout_secs` 为 90
  - 刷新 WebUI 页面后，Runtime Settings 中 Shell Timeout 显示为 90

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

### TC-IMA-29: Agent Tab 数据目录统一 - 旧目录兼容加载

- **操作步骤**:
  1. 确保 `~/.bifrost-agent/config.toml` 存在（旧位置）
  2. 启动 Bifrost，检查启动日志
  3. 通过 GET API 获取配置
- **预期结果**:
  - 启动日志包含 `loaded legacy user-level config path=.../.bifrost-agent/config.toml`
  - GET API 返回的配置包含 TOML 中的设置（如 model、work_dir、mcp_servers）
  - AgentConfigStore 的 JSON 文件存储在 `~/.bifrost/agent/agent_config.json`

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
  2. 验证以下 Go DefaultModelConfig() 对应的字段均已正确配置
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
  - 每个 provider 包含 `id`、`name`、`base_url`、`env_key` 字段
  - 包含的 provider ID: `openai`, `aidp_crawl`, `azure`, `anthropic`, `gemini`, `groq`, `deepseek`, `ollama`, `lmstudio`, `amazon-bedrock`, `openrouter`, `xai`, `mistral`, `cerebras`
  - `openai.base_url` = `"https://api.openai.com/v1/chat/completions"`
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

### TC-IMA-37: Session 管理 - Active Sessions 列表展示

- **操作步骤**:
  1. 打开 Settings → Agent Tab
  2. 滚动到 "Active Sessions" 区域
  3. 确认表格显示活跃会话列表（Session Key, Messages, Tokens, Created, Last Active, Actions 列）
  4. 点击 Refresh 按钮
- **预期结果**:
  - 表格正确渲染，无 JS 错误
  - 如果有活跃会话，显示在表格中
  - 如果没有活跃会话，显示 "No active sessions" 空状态
  - Refresh 按钮可正常刷新列表

### TC-IMA-38: Session 管理 - Session History 列表展示

- **操作步骤**:
  1. 打开 Settings → Agent Tab
  2. 滚动到 "Session History" 区域
  3. 确认表格显示持久化会话文件列表（Session Key, Created, Filename, Actions 列）
  4. 点击 Refresh 按钮
- **预期结果**:
  - 表格正确渲染，无 JS 错误
  - 如果有持久化会话文件，显示在表格中
  - 如果没有持久化会话文件，显示 "No persisted sessions" 空状态
  - Refresh 按钮可正常刷新列表

### TC-IMA-39: Session 管理 - Active Session 详情查看

- **操作步骤**:
  1. 先通过 API 创建一个会话：
  ```bash
  curl -s -X POST http://127.0.0.1:8800/_bifrost/api/im-gateway/agent/chat \
    -H 'Content-Type: application/json' \
    -d '{"session_key": "ui-test", "message": "hello"}'
  ```
  2. 打开 Agent Tab → Active Sessions，点击 Refresh
  3. 点击 "ui-test" 会话的查看按钮（眼睛图标）
- **预期结果**:
  - Modal 弹出显示 Session Detail
  - 包含 Session Key、History Version、Created、Last Active、Compactions 元信息
  - Messages 区域显示会话消息，角色标签颜色区分（user=绿色, assistant=蓝色, system=紫色）

### TC-IMA-40: Session 管理 - Active Session 删除

- **操作步骤**:
  1. 在 Active Sessions 表格中找到 "ui-test" 会话
  2. 点击删除按钮（垃圾桶图标）
  3. 确认弹窗中点击确认
- **预期结果**:
  - 显示 "Session deleted" 成功提示
  - 表格自动刷新，"ui-test" 不再出现
  - API 验证：`curl http://127.0.0.1:8800/_bifrost/api/im-gateway/agent/sessions` 不包含 "ui-test"

### TC-IMA-41: Session History API - 列出持久化文件

- **操作步骤**:
  ```bash
  curl -s http://127.0.0.1:8800/_bifrost/api/im-gateway/agent/sessions/history | jq .
  ```
- **预期结果**:
  - 返回 JSON 包含 `history` 数组和 `total` 计数
  - 每个 history 项包含 `path`, `filename`, `session_key`, `timestamp` 字段

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
     - Skills
     - AGENTS.md Instructions
     - Active Sessions
     - Session History
- **预期结果**:
  - 所有 10 个 Card 区域完整渲染，无缺失
  - 各配置项数值正确显示（非空，有默认值）
  - 无控制台 JS 错误

### TC-IMA-43: 组件拆分验证 - 暗色主题兼容性

- **操作步骤**:
  1. 切换到暗色主题
  2. 打开 Settings → Agent Tab
  3. 检查 Active Sessions 和 Session History 区域
- **预期结果**:
  - 表格、按钮、Modal 在暗色主题下颜色正确
  - Session Detail Modal 中的消息卡片颜色区分清晰

## 动态工作目录与 Session 管理

### TC-IMA-44: 创建带 work_dir 的 Session

- **操作步骤**:
  ```bash
  curl -s -X POST http://127.0.0.1:8800/_bifrost/api/im-gateway/agent/chat \
    -H 'Content-Type: application/json' \
    -d '{"session_key": "wd-test-1", "work_dir": "/Users/eden/work/github/bifrost", "message": "hi"}'
  ```
- **预期结果**:
  - 返回 `success: true`
  - GET /sessions 中 `wd-test-1` 的 `work_dir` 为 `/Users/eden/work/github/bifrost`

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
    -d '{"session_key": "wd-switch-test", "work_dir": "/Users/eden", "message": "请切换工作目录到 /Users/eden/work/github/bifrost"}'
  ```
- **预期结果**:
  - `tool_calls` 中包含 `switch_workdir` 工具调用，`success: true`
  - `response` 包含 "已切换工作目录到"
  - GET /sessions 中该 session 的 `work_dir` 更新为 `/Users/eden/work/github/bifrost`
  - `message_count` 为 0（历史已清空）

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

### TC-IMA-54: 正常对话 - 确认错误处理不影响正常流程

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
