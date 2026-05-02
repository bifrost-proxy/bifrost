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
