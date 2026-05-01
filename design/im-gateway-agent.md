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

### 5. handle_agent_chat() - 主入口函数

**处理流程**：
1. 查找或创建用户会话
2. 构建消息列表（历史 + 当前消息）
3. 调用模型 API
4. 记录对话轮次到会话历史
5. 返回模型响应

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
│   │   ├── agent.rs          # Agent 核心实现
│   │   │   ├── ImAgentConfig
│   │   │   ├── ImAgentConfigStore
│   │   │   ├── ImAgentClient
│   │   │   ├── ImAgentSessionManager
│   │   │   └── handle_agent_chat()
│   │   ├── types.rs          # ImRouteAction::AgentChat 变体
│   │   └── mod.rs
│   └── handlers/
│       └── im_gateway.rs     # HTTP Handler 集成
│           ├── get_agent_config()
│           ├── update_agent_config()
│           └── list_agent_sessions()
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

### 真实场景测试（human_tests）

**测试用例文档**：`human_tests/im-gateway-agent.md`

| 用例编号 | 用例名称 | 验证点 |
|----------|----------|--------|
| TC-AG-01 | 基础对话 | 飞书发送消息 → 收到回复 |
| TC-AG-02 | 多轮对话 | 连续对话 → 上下文关联 |
| TC-AG-03 | 会话清空 | /clear → 历史清空 |
| TC-AG-04 | 路由覆盖 | 触发 AgentChat 路由 → 使用自定义配置 |
| TC-AG-05 | 非_OWNER_拦截 | 非 owner 用户 → 无响应 |
| TC-AG-06 | 配置更新 | 通过 API 更新配置 → 生效 |

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
