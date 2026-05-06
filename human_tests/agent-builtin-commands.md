# Agent 内置命令全面测试

## 功能模块说明

Agent 内置 11 个斜杠命令（/help、/clear、/reset、/undo、/compact、/status、/resume、/remember、/memories、/forget、/skill），通过 `POST /_bifrost/api/im-gateway/agent/chat` 或 IM 消息触发。本文档覆盖每个命令的正常路径、边界条件和错误处理。

## 前置条件

1. 启动 Bifrost 服务：
```bash
BIFROST_DATA_DIR=./.bifrost-cmd-test cargo run --bin bifrost -- start -p 8801 --unsafe-ssl --no-system-proxy
```
2. 确认 Agent 已启用（`GET /_bifrost/api/im-gateway/agent` 返回 `enabled: true`）
3. API 基础 URL：`http://127.0.0.1:8801/_bifrost/api/im-gateway/agent/chat`
4. 请求格式：`POST` + `Content-Type: application/json` + `{"message": "...", "session_key": "..."}`

## 测试用例

### TC-BC-01: /help 返回完整帮助信息

**操作步骤**：
1. 发送 `{"message": "/help", "session_key": "tc-bc-01"}`
2. 检查响应内容

**预期结果**：
- `success: true`
- response 包含"可用命令:"
- response 包含"内置命令:"
- response 列出所有 11 个内置命令：/clear、/compact、/forget、/help、/memories、/remember、/reset、/resume、/skill、/status、/undo
- 每个命令附带中文描述
- response 包含"提示: 直接输入文本即可与 AI 对话。"

### TC-BC-02: /status 显示会话状态

**操作步骤**：
1. 发送 `{"message": "/status", "session_key": "tc-bc-02"}`

**预期结果**：
- `success: true`
- response 包含"会话状态:"
- response 包含"工作路径:"
- response 包含"消息数: 0"（新会话）
- response 包含"估算 token"、"API 累计 token"、"压缩次数"、"历史版本"、"MCP 工具数"

### TC-BC-03: /clear 清除会话历史

**操作步骤**：
1. 发送 `{"message": "/clear", "session_key": "tc-bc-03"}`

**预期结果**：
- `success: true`
- response 为"会话已重置，可以开始新的对话。"

### TC-BC-04: /reset 行为与 /clear 一致

**操作步骤**：
1. 发送 `{"message": "/reset", "session_key": "tc-bc-04"}`

**预期结果**：
- `success: true`
- response 为"会话已重置，可以开始新的对话。"

### TC-BC-05: /undo 默认回退 1 轮

**操作步骤**：
1. 发送 `{"message": "/undo", "session_key": "tc-bc-05"}`

**预期结果**：
- `success: true`
- response 包含"已回退 1 轮对话"
- response 包含"当前历史:"和"条消息"

### TC-BC-06: /undo N 回退指定轮数

**操作步骤**：
1. 发送 `{"message": "/undo 3", "session_key": "tc-bc-06"}`

**预期结果**：
- `success: true`
- response 包含"已回退 3 轮对话"

### TC-BC-07: /compact 在历史太少时提示无需压缩

**操作步骤**：
1. 发送 `{"message": "/compact", "session_key": "tc-bc-07"}`（新会话，无历史）

**预期结果**：
- `success: true`
- response 为"历史消息太少，无需压缩。"

### TC-BC-08: /remember 保存长期记忆

**操作步骤**：
1. 发送 `{"message": "/remember 这是一条测试记忆", "session_key": "tc-bc-08"}`

**预期结果**：
- `success: true`
- response 包含"已记住长期记忆:"
- response 包含记忆 ID

### TC-BC-09: /remember 无参数提示用法

**操作步骤**：
1. 发送 `{"message": "/remember", "session_key": "tc-bc-09"}`

**预期结果**：
- `success: true`
- response 为"用法: /remember <text>"

### TC-BC-10: /memories 列出可见长期记忆

**操作步骤**：
1. 先发送 `/remember 测试记忆内容`（确保有记忆存在）
2. 发送 `{"message": "/memories", "session_key": "tc-bc-10"}`

**预期结果**：
- `success: true`
- response 包含"当前可见长期记忆文件条目:"或"当前 scope 没有长期记忆。"
- 如果之前有 remember 过内容，应能看到相关记忆条目

### TC-BC-11: /forget 无参数提示用法

**操作步骤**：
1. 发送 `{"message": "/forget", "session_key": "tc-bc-11"}`

**预期结果**：
- `success: true`
- response 为"用法: /forget <id|last>"

### TC-BC-12: /forget last 删除最近的记忆

**操作步骤**：
1. 先发送 `/remember 要删除的记忆`
2. 发送 `{"message": "/forget last", "session_key": "tc-bc-12"}`

**预期结果**：
- `success: true`
- response 包含"已忘记长期记忆:"或"没有找到可忘记的长期记忆。"

### TC-BC-13: /resume 在无历史时提示无记录

**操作步骤**：
1. 发送 `{"message": "/resume", "session_key": "completely-new-session-xyz"}`（从未使用过的 session key）

**预期结果**：
- `success: true`
- response 为"没有找到可恢复的会话记录。"

### TC-BC-14: /resume 恢复有历史的会话

**操作步骤**：
1. 先用 session_key="tc-bc-14" 发送至少一条普通消息或 `/remember` 命令
2. 重启服务（或等 session 过期）
3. 用相同 session_key 发送 `{"message": "/resume", "session_key": "tc-bc-14"}`

**预期结果**：
- `success: true`
- response 包含"已恢复会话历史，加载了"和"条消息"
- 消息数 > 0

### TC-BC-15: /skill 返回 Skill Creator 提示

**操作步骤**：
1. 发送 `{"message": "/skill", "session_key": "tc-bc-15"}`

**预期结果**：
- `success: true`
- response 为"Skill Creator 已启动。请描述要创建或编辑的 skill。"

### TC-BC-16: 未知命令返回错误提示

**操作步骤**：
1. 发送 `{"message": "/nonexistent_cmd", "session_key": "tc-bc-16"}`

**预期结果**：
- `success: true`
- response 为"未知命令: /nonexistent_cmd"

### TC-BC-17: Session-free 命令在 session 忙碌时立即响应

**操作步骤**：
1. 发送一条需要 LLM 处理的普通消息（如 "Tell me a story"），使用 session_key="tc-bc-17"
2. 在该请求处理期间（1 秒后），发送 `/help`，使用相同 session_key
3. 在该请求处理期间，发送 `/remember busy_test`，使用相同 session_key

**预期结果**：
- `/help` 立即返回帮助信息（不等待第一个请求完成）
- `/remember` 立即返回"已记住长期记忆: ..."

### TC-BC-18: Session-required 命令在 session 忙碌时返回忙碌提示

**操作步骤**：
1. 发送一条需要 LLM 处理的普通消息（如 "Tell me a story"），使用 session_key="tc-bc-18"
2. 在该请求处理期间（1 秒后），发送 `/status`，使用相同 session_key
3. 在该请求处理期间，发送 `/clear`，使用相同 session_key

**预期结果**：
- `/status` 返回"⏳ Agent 正在处理中，请稍后再试。"
- `/clear` 返回"⏳ Agent 正在处理中，请稍后再试。"
- 提示中包含 session-free 命令列表

### TC-BC-19: Session 忙碌结束后恢复正常

**操作步骤**：
1. 发送一条需要 LLM 处理的普通消息，使用 session_key="tc-bc-19"
2. 等待请求完成
3. 发送 `/status`，使用相同 session_key

**预期结果**：
- `/status` 正常返回会话状态信息（不再返回忙碌提示）

### TC-BC-20: /status 在 Agent Loop 运行中返回实时指标

**操作步骤**：
1. 使用临时数据目录启动 Bifrost：`BIFROST_DATA_DIR=./.bifrost-cmd-test cargo run --bin bifrost -- start -p 8801 --unsafe-ssl --no-system-proxy`
2. 将 Agent 配置为一个会延迟 3 秒返回的 mock Chat Completions provider。
3. 发送一条普通消息，使用 session_key="tc-bc-20"，保持请求仍在执行中。
4. 在第 3 步请求未完成时，发送 `{"message": "/status", "session_key": "tc-bc-20"}`。
5. 等待第 3 步请求完成，再次发送 `/status`。

**预期结果**：
- 运行中 `/status` 返回 `success: true`
- response 包含"会话状态:"、"正在处理中"、"工作路径:"、"Loop:"、"实时 token:"、"Context 用量:"、"压缩次数:"
- response 不再只是"Agent 正在处理中，请稍后再试。"
- JSON 响应包含 `active_status` 对象，且 `active_status.current_loop_iteration >= 1`
- `active_status.work_dir` 等于当前 session 的工作目录（未显式传入时允许为 `null`）
- `active_status.max_loop_iterations` 等于当前 Agent 配置的迭代上限
- 未显式配置 `model_context_window` 时，`active_status.context_window_tokens` 等于 `250000`
- `active_status.context_usage_percent` 为可读数值或 `null`（仅当未配置 context window 时允许为 `null`）
- 忙碌结束后的 `/status` 返回空闲会话状态，包含"API 累计 token"与"Context 用量"

## 清理步骤

1. 停止 Bifrost 服务
2. 删除临时数据目录：`rm -rf ./.bifrost-cmd-test`
