# MCP Servers Settings

## 功能模块描述

Settings -> Agent -> MCP Servers 用于管理 Agent 可使用的 MCP server 配置。列表必须同时展示配置和运行可用性，避免用户只能看到已配置项，却不知道实际是否能启动、握手并枚举工具。

## 实现逻辑

- 后端新增只读接口 `GET /_bifrost/api/im-gateway/agent/mcp-status`。
- 接口读取当前 Agent 配置中的 `mcp_servers`，对所有已配置 server 生成状态：
  - `available`：enabled 且启动、initialize、`tools/list` 成功，返回 `tool_count`。
  - `unavailable`：enabled 但启动、连接、initialize 或 `tools/list` 失败，返回错误摘要。
  - `disabled`：配置存在但 `enabled=false`，不启动进程或 HTTP 连接。
- 检查逻辑复用 Agent MCP runtime 的启动路径，保证 Settings 页看到的状态与真实 Agent turn 使用的 MCP 初始化路径一致。
- WebUI MCP Servers section 挂载时自动请求状态接口；请求期间展示 `Checking`，返回后按 server 名称展示状态 tag、tool count 或错误信息，并提供 Refresh 手动复查。

## 依赖项

- `crates/agent/src/mcp/mod.rs`：复用 MCP 启动、握手、tools/list。
- `crates/bifrost-admin/src/handlers/im_gateway/agent_api.rs`：`/agent/mcp-status` 路由实现（父分发器位于 `crates/bifrost-admin/src/handlers/im_gateway.rs`）。
- `web/src/pages/Settings/tabs/agent/McpServersSection.tsx`：MCP Servers 列表展示。

## 测试方案

- 单元测试：验证 disabled server 返回 `disabled` 且不启动；缺少 command/url 的 enabled server 返回 `unavailable` 和错误信息。
- E2E/UI 测试：拦截 Agent 配置和 `mcp-status` API，进入 MCP Servers section 后断言页面自动请求状态并显示 available、unavailable、disabled 差异。
- 真实场景测试：`human_tests/mcp-servers.md` 覆盖页面进入自动检查、亮色/暗色状态可读、手动刷新与失败错误展示。

## Review/Fix/Test 闭环方案

- 第 1 轮：复核用户目标、后端状态枚举、WebUI 自动触发时机和测试覆盖；运行 MCP 单元测试与 UI spec。
- 第 2 轮：基于最新 diff 复查文档、human_tests 索引、失败/disabled 边界与主题可读性；复跑受影响测试。
- 如任一轮发现功能缺口、测试失败或文档不同步，追加下一轮直到关闭。

## 校验要求

- 先执行受影响 MCP 单元测试和 WebUI E2E/UI 测试。
- 再执行 `rust-project-validate` 要求的 fmt、clippy、受影响测试和 `cargo test --workspace --all-features`。
- `scripts/ci/local-ci.sh` 仅在最终范围需要完整本地 CI 时执行；若成本过高或范围不适用，最终交付说明原因和风险。

## 文档更新要求

- 更新 `human_tests/mcp-servers.md`。
- 更新 `human_tests/readme.md` 索引。
- 本次不新增外部用户命令或配置字段，不需要 README 协议/Hook/CLI 文档更新。
