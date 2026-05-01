# Agent Session 持久化测试

## 功能模块说明

验证 Agent Session 的 JSONL 持久化功能：
- 通过飞书机器人或 `/agent/chat` API 发送消息后，session 事件自动写入 `~/.bifrost/agent/sessions/` 目录的 JSONL 文件
- JSONL 文件包含完整执行过程（session_start、user_message、tool_call、tool_result、assistant_message、compaction、session_end 等）
- WebUI 可查看 session 历史文件列表、查看详细事件时间线、删除 session 文件
- 跨 turn 复用同一 recorder（同一 session 多次对话写入同一文件）
- 受 `ephemeral` 和 `history.persistence` 配置控制

## 前置条件

1. 编译并启动 Bifrost 服务（临时数据目录）：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --unsafe-ssl --no-system-proxy
   ```
2. 确保 Agent 功能已启用（`enabled: true`）
3. 确保 `ephemeral` 为 `false`，`history.persistence` 为 `SaveAll`（默认即可）
4. 清空 `~/.bifrost/agent/sessions/` 目录以便观察新生成文件

## 测试用例

### TC-ASP-01：通过 /agent/chat API 发送消息后生成 JSONL 文件

**操作步骤**：
1. 调用 `POST http://localhost:8800/_bifrost/agent/chat`，body: `{"message": "hello, what is 1+1?", "session_key": "test-persist-01"}`
2. 等待响应返回
3. 检查 `~/.bifrost/agent/sessions/` 目录是否生成了包含 `test-persist-01` 的 JSONL 文件

**预期结果**：
- API 返回 `{ "success": true, "response": "..." }`
- `~/.bifrost/agent/sessions/` 目录下生成 `session-test-persist-01-*.jsonl` 文件

### TC-ASP-02：JSONL 文件包含 session_start 事件

**操作步骤**：
1. 读取 TC-ASP-01 生成的 JSONL 文件
2. 解析第一行 JSON

**预期结果**：
- 第一行的 `event_type` 为 `session_start`
- `session_key` 为 `test-persist-01`
- `content` 包含 `model` 和 `provider` 字段

### TC-ASP-03：JSONL 文件包含 user_message 事件

**操作步骤**：
1. 读取 TC-ASP-01 生成的 JSONL 文件
2. 查找 `event_type` 为 `user_message` 的行

**预期结果**：
- 存在 `user_message` 事件
- `content` 包含用户发送的消息文本 `"hello, what is 1+1?"`

### TC-ASP-04：JSONL 文件包含 assistant_message 事件

**操作步骤**：
1. 读取 TC-ASP-01 生成的 JSONL 文件
2. 查找 `event_type` 为 `assistant_message` 的行

**预期结果**：
- 存在 `assistant_message` 事件
- `content` 包含 Agent 的回复文本

### TC-ASP-05：跨 turn 复用同一 JSONL 文件

**操作步骤**：
1. 使用相同 session_key 再次调用 `POST /agent/chat`，body: `{"message": "and what is 2+2?", "session_key": "test-persist-01"}`
2. 检查 `~/.bifrost/agent/sessions/` 目录

**预期结果**：
- 不会生成新的 JSONL 文件（仍然只有一个 `session-test-persist-01-*.jsonl`）
- 文件内容追加了第二轮对话的 user_message 和 assistant_message 事件

### TC-ASP-06：GET /agent/sessions/history 列表 API

**操作步骤**：
1. 调用 `GET http://localhost:8800/_bifrost/agent/sessions/history`

**预期结果**：
- 返回 `{ "history": [...], "total": N }`
- history 数组中包含至少一个条目，含 `path`、`filename`、`session_key`、`timestamp` 字段
- `session_key` 为 `test-persist-01`

### TC-ASP-07：GET /agent/sessions/history/{path} 详情 API 返回完整事件

**操作步骤**：
1. 从 TC-ASP-06 的结果中取出 `path` 字段（URL encode）
2. 调用 `GET http://localhost:8800/_bifrost/agent/sessions/history/{encoded_path}`

**预期结果**：
- 返回 `{ "events": [...], "count": N }`
- events 数组包含 `session_start`、`user_message`、`assistant_message` 等事件类型
- 每个事件包含 `timestamp`、`event_type`、`session_key`、`content` 字段

### TC-ASP-08：DELETE /agent/sessions/history/{path} 删除 session 文件

**操作步骤**：
1. 从 TC-ASP-06 的结果中取出 `path` 字段
2. 调用 `DELETE http://localhost:8800/_bifrost/agent/sessions/history/{encoded_path}`
3. 确认文件已删除

**预期结果**：
- 返回 `{ "ok": true }`
- 再次调用 GET 列表 API，该 session 不再出现

### TC-ASP-09：WebUI Session History 列表展示

**操作步骤**：
1. 先通过 API 再创建一个 session 确保有数据
2. 在浏览器中打开 `http://localhost:8800/_bifrost/` 进入 Settings > Agent Tab
3. 找到 Session History 区域

**预期结果**：
- 表格展示持久化的 session 文件列表
- 每行显示 session key、时间戳等信息
- 有"查看"和"删除"操作按钮

### TC-ASP-10：WebUI 查看 Session 详情事件时间线

**操作步骤**：
1. 在 Session History 列表中点击某条 session 的"查看"按钮
2. 弹出详情模态框

**预期结果**：
- 模态框展示事件时间线
- 不同事件类型有不同的视觉样式（颜色、图标）
- session_start 显示 model/provider 信息
- user_message 显示用户消息内容
- assistant_message 显示 Agent 回复内容
- tool_call 显示工具名和参数（如有）
- tool_result 显示执行结果和成功/失败状态（如有）

### TC-ASP-11：WebUI 删除 Session 文件

**操作步骤**：
1. 在 Session History 列表中点击某条 session 的"删除"按钮
2. 确认删除

**预期结果**：
- session 从列表中消失
- 对应的 JSONL 文件被删除

### TC-ASP-12：暗色主题兼容性

**操作步骤**：
1. 切换到暗色主题
2. 查看 Session History 列表和详情模态框

**预期结果**：
- 所有文本、卡片、标签在暗色主题下清晰可辨
- 事件卡片颜色适配暗色主题

## 清理步骤

1. 停止 Bifrost 服务
2. 删除临时数据目录：`rm -rf ./.bifrost-test`
3. 清理测试生成的 session 文件（如果在默认目录）：`rm -f ~/.bifrost/agent/sessions/session-test-persist-*`
