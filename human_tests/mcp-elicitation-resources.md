# MCP Elicitation 和 Resources 协议模块测试

## 功能模块说明

MCP Elicitation 和 Resources 协议模块为 Bifrost Agent 提供：
1. **Elicitation 协议**：处理 MCP 服务器向客户端请求用户输入的流程，包含策略决策（自动拒绝/交互式）、回调处理、暂停状态追踪。
2. **Resources 协议**：管理 MCP 服务器暴露的资源（文件、数据库 schema 等），提供列表/读取操作、缓存管理、虚拟工具定义。

## 前置条件

1. 工作目录：`/Users/eden/work/github/bifrost`
2. 文件已创建：
   - `crates/agent/src/mcp/elicitation.rs`
   - `crates/agent/src/mcp/resources.rs`
3. `crates/agent/src/mcp/mod.rs` 中已声明 `pub mod elicitation;` 和 `pub mod resources;`

## 测试用例

### TC-MER-01: 新文件语法正确性验证

**操作步骤**：
1. 执行 `cargo check -p bifrost-agent 2>&1 | grep -E "(elicitation\.rs|resources\.rs)"`

**预期结果**：
- 无任何输出（即新文件没有编译错误）

### TC-MER-02: elicitation.rs 类型序列化验证

**操作步骤**：
1. 确认 `ElicitationRequest` 能正确序列化/反序列化 JSON 字段名（`requestedSchema`, `elicitationId`）
2. 确认 `ElicitationAction` 的三个变体序列化为 `"accept"`, `"decline"`, `"cancel"`
3. 检查 elicitation.rs 中的测试：`test_elicitation_action_all_variants_serialize` 和 `test_elicitation_request_serialization_roundtrip`

**预期结果**：
- 类型使用 `super::types::` 引用已有类型定义
- 测试覆盖全部三个 action 变体
- 序列化使用 `#[serde(rename_all = "camelCase")]` 风格

### TC-MER-03: ElicitationPolicy 策略判断逻辑

**操作步骤**：
1. 检查 `ElicitationPolicy::from_approval_mode("auto")` → `AutoDecline`
2. 检查 `ElicitationPolicy::from_approval_mode("never")` → `AutoDecline`
3. 检查 `ElicitationPolicy::from_approval_mode("prompt")` → `Interactive`
4. 检查 `ElicitationPolicy::from_approval_mode("")` → `Interactive`
5. 确认 `Default` 实现返回 `AutoDecline`

**预期结果**：
- 所有断言在测试代码中正确实现
- 源码中 `from_approval_mode` 方法使用明确的 match 分支

### TC-MER-04: ElicitationHandler AutoDecline 行为

**操作步骤**：
1. 检查 `test_handler_auto_decline_returns_decline` 测试逻辑
2. 确认 AutoDecline 策略下直接返回 Decline 且不调用回调

**预期结果**：
- handler 构造时 policy=AutoDecline 无需 callback
- `handle()` 返回 `ElicitationAction::Decline` 且 content=None

### TC-MER-05: ElicitationHandler Interactive 回调流程

**操作步骤**：
1. 检查 `test_handler_interactive_with_callback_accept` 测试
2. 确认 callback 返回的 oneshot::Receiver 被正确 await
3. 检查 `test_handler_interactive_callback_dropped_returns_cancel` 确认通道关闭时返回 Cancel

**预期结果**：
- callback 正常响应时返回用户选择的 action 和 content
- callback 通道异常关闭时返回 Cancel

### TC-MER-06: ElicitationPauseState RAII 守卫

**操作步骤**：
1. 检查 `test_pause_guard_auto_resumes` 测试
2. 确认 `enter()` 设置 paused=true
3. 确认 guard drop 时自动设置 paused=false
4. 确认 clone 共享状态

**预期结果**：
- `ElicitationPauseGuard` 实现 `Drop` trait 自动 resume
- 多个 clone 共享同一 AtomicBool

### TC-MER-07: resources.rs 工具定义生成

**操作步骤**：
1. 检查 `list_resources_tool_def()` 返回的 JSON 结构
2. 确认 name 为 `"mcp__list_resources"`
3. 确认 parameters 包含 `server` 和 `cursor` 属性
4. 检查 `read_resource_tool_def()` 的 required 包含 `["server", "uri"]`
5. 确认 `all_resource_tool_defs()` 返回 3 个定义

**预期结果**：
- 工具名称符合 `mcp__` 前缀约定
- read_resource 要求 server 和 uri 为必填
- list 类工具无必填参数

### TC-MER-08: McpRequestSender trait 解耦验证

**操作步骤**：
1. 确认 `McpRequestSender` trait 使用 `#[async_trait::async_trait]` 标注
2. 确认 trait 方法签名：`send_request(&self, method: &str, params: Option<Value>) -> Result<Value, String>`
3. 确认 ResourcesManager 的方法通过 `&dyn McpRequestSender` 接收 sender

**预期结果**：
- resources.rs 不直接 import McpConnection 或 mod.rs 中的类型
- 通过 trait object 实现解耦
- Mock 实现在测试中正确工作

### TC-MER-09: ResourcesManager 缓存行为

**操作步骤**：
1. 检查 `test_cache_populated_on_list` 测试：首次 list 后缓存被填充
2. 检查 `test_cache_not_updated_with_cursor`：带 cursor 的分页请求不更新缓存
3. 检查 `test_invalidate_cache`：invalidate 清除指定 server 缓存
4. 检查 `test_clear_cache`：clear 清除全部缓存

**预期结果**：
- 缓存仅在无 cursor 的首页请求时更新
- invalidate 只清除指定 server
- clear 清除所有 server 的缓存

### TC-MER-10: ResourcesManager 错误处理

**操作步骤**：
1. 检查 `test_list_resources_error`：sender 返回错误时 list 返回 Err
2. 检查 `test_list_all_resources_handles_errors_gracefully`：部分 server 失败不影响其他 server

**预期结果**：
- 单个 server 的错误被传播为 Err
- list_all_resources 中失败的 server 被跳过（warn 日志），成功的仍返回

### TC-MER-11: 模块声明正确性

**操作步骤**：
1. 读取 `crates/agent/src/mcp/mod.rs` 前 40 行
2. 确认包含 `pub mod elicitation;` 和 `pub mod resources;`

**预期结果**：
- 两个模块均为 pub 声明
- 位于其他已有模块声明旁边

## 清理步骤

无需清理（纯代码模块，无运行时副作用）。
