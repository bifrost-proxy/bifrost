# MCP Servers 可用性状态设计方案

## 背景

Bifrost Agent 通过 `mcp_servers` 配置对接外部 MCP (Model Context Protocol) server：本地进程（stdio）、HTTP、SSE 三种传输方式共享同一份运行时。Agent 每次开启新会话都要启动、握手、`tools/list`，才能把这些 server 的 tool 暴露给模型。

在这一节 UI 之前，`Settings -> Agent -> MCP Servers` 只做配置读写。用户只能看到自己配了哪些 server，却看不到进程能不能真正拉起、`initialize` 会不会握手失败、`tools/list` 是否为空。结果只能等到实际发对话、模型报错才知道 MCP 挂了，排障成本高。

新方案在 Settings 页挂载 MCP Servers section 时自动请求后端只读接口 `/agent/mcp-status`，用与 Agent turn 完全相同的 runtime 路径去启动握手每个 enabled server，并把 `available`/`unavailable`/`disabled` 三态回填到 UI，附上 `tool_count` 或错误摘要，实现"配置即诊断"。

## 用户目标验证清单

### 必须实现

- Settings -> Agent -> MCP Servers section 挂载时**自动**请求 `/agent/mcp-status`，无需用户点按钮。
- 每个 enabled server 都用真实 runtime 启动进程/建 HTTP/SSE 连接、执行 `initialize`、执行 `tools/list`，并给出成功/失败结论。
- 状态分三档：`available`（附 `tool_count`）、`unavailable`（附 `error` 摘要）、`disabled`（`enabled=false`，不启动进程/连接）。
- 检查过程中在对应卡片显示 `Checking` loading tag；返回后按状态渲染 tag。
- 提供 `Refresh Status` 手动重新检查按钮。
- 状态检查路径必须复用 `bifrost_agent::mcp` 的启动/握手实现，避免"UI 显示可用但实际会话仍然握手失败"。
- HTTP/SSE 类型的 server 检查时必须遵循 Agent 使用的 model proxy 与 CA 证书策略，避免出口配置不一致导致误判。

### 必须不破坏

- MCP Servers 配置的增删改保存流程（`PATCH /agent/config`）保持不变；状态检查是纯只读旁路。
- 状态接口失败不阻塞用户编辑或保存 server 配置。
- Disabled server **不**启动子进程或连接，避免误消耗资源、误触发 OAuth 登录。
- 状态检查不阻塞 Settings tab 切换或页面渲染。
- 亮/暗色主题下 tag 颜色可读，符合现有 Ant Design token 语义。

### 必须真实验证

- Playwright UI 测试断言：进入 `agentSection=mcp-servers` 后 `mcp-status` 请求次数 >= 1，`filesystem` 展示 `Available`、`broken` 展示 `Unavailable`、`disabled` 展示 `Disabled`。
- 单元测试断言：disabled server 不启动子进程，enabled 但配置非法的 server 返回 `Unavailable` 并带错误摘要。
- 后端集成测试断言：`GET /agent/mcp-status` 返回 `{ "servers": [...] }` 结构，`POST` 返回 405。

## 产品语义

### 状态三态

MCP server 的可用性只有三态，来自后端枚举 `McpServerAvailabilityStatus`：

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum McpServerAvailabilityStatus {
    Available,
    Unavailable,
    Disabled,
}
```

- `available`：`enabled = true`，能够启动/建立连接、`initialize` 成功、`tools/list` 拿到工具列表（可以为 0 但不出错）。UI 展示绿色 tag + `Tools: N`。
- `unavailable`：`enabled = true`，任一环节失败（找不到可执行文件、握手超时、协议错误、HTTP 4xx/5xx、CA 校验失败等）。UI 展示红色 tag + 错误文案，用于用户复盘。
- `disabled`：`enabled = false`，后端**跳过**启动，直接把 status 置为 disabled、`tool_count = 0`、`error = None`。UI 展示灰色 tag。

### 检查复用 Agent runtime

状态检查绝对不能开一条"检查专用"的启动路径，否则会与 Agent turn 分叉：Agent 会话里 MCP 用一套 timeout/env/cwd/http 网络策略，UI 用另一套，就会出现"UI 说可用，实际不能用"或反过来。所以入口都收敛到：

```rust
pub async fn check_server_availability(configs) -> Vec<McpServerAvailability>;
pub async fn check_server_availability_with_http_network(configs, http_network) -> Vec<McpServerAvailability>;
```

内部并发启动、复用 `start_one_server`（这也是 Agent runtime 拉起 server 的函数），完成后立刻断开连接，不落库。

### 出口网络与 CA 一致性

Agent 出站请求可以走 Bifrost 自己的模型代理并信任本地 CA。MCP HTTP/SSE server 走的是同一网络语义，因此 `agent_api.rs` 里状态检查需要：

- 从 `agent_client.model_proxy_url()` 读出代理地址（可能是 `None`）。
- 若有代理，构造 `McpHttpNetwork::with_proxy_and_ca(proxy_url, Some(data_dir/certs/ca.crt))`。
- 若没有代理，使用 `McpHttpNetwork::direct()`。

任何绕过这个网络策略的自制 HTTP client 都会导致状态检查与实际 Agent turn 不一致，视为回归。

## 技术细节

### 关键文件

- 后端 runtime：`crates/agent/src/mcp/mod.rs`
  - `McpServerAvailability`、`McpServerAvailabilityStatus`、`check_server_availability_with_http_network`（`mod.rs:689-799`）
  - `start_one_server`：真实 stdio/HTTP/SSE 启动与握手入口。
- Admin API 路由：`crates/bifrost-admin/src/handlers/im_gateway/agent_api.rs`
  - `GET /agent/mcp-status` 分支实现在 `agent_api.rs:154-176`
  - 读取 `agent_config_store` 中的 `mcp_servers`，构造 `McpHttpNetwork`，调用 runtime。
- 父路由分发：`crates/bifrost-admin/src/handlers/im_gateway.rs`
- 前端组件：`web/src/pages/Settings/tabs/agent/McpServersSection.tsx`
  - `refreshAvailability`：调用 `GET ${BASE}/agent/mcp-status`，写入 `availability` state。
  - `useEffect(..., [refreshAvailability])`：section 挂载即拉一次。
  - `renderAvailability`：三态 tag 渲染 + `Checking` loading + `Tools: N` + 错误文案。
  - `Refresh Status` 按钮 → `refreshAvailability()`。

### 响应结构

```json
GET /_bifrost/api/im-gateway/agent/mcp-status
200 OK
{
  "servers": [
    { "name": "broken",     "enabled": true,  "status": "unavailable", "tool_count": 0, "error": "command not found" },
    { "name": "disabled",   "enabled": false, "status": "disabled",    "tool_count": 0 },
    { "name": "filesystem", "enabled": true,  "status": "available",   "tool_count": 3 }
  ]
}
```

- `servers` 按 `name` 字典序排序，保证 UI 呈现顺序稳定，便于 UI 快照测试比对。
- `POST/PUT/DELETE /agent/mcp-status` 返回 405 Method Not Allowed。

### 并发与性能

- Runtime 用 `futures::stream::buffer_unordered(STARTUP_CONCURRENCY)` 并发启动，避免多个 server 逐个握手。
- 单个 server 的启动超时沿用 MCP 默认 timeout 常量（`test_mcp_default_timeout_constants_match_runtime_policy`）。
- 检查完成后立即释放子进程/HTTP 连接，不残留 idle 会话。

### 前端交互细节

- Section 挂载或 servers key 集合变化 → 自动 refresh。
- `checking` 期间对未拿到结果的 server 显示 `Checking` tag；已经拿到旧结果的 server 保持旧 tag，避免闪烁为空。
- 请求失败：置 `checkError = "Failed to check MCP server availability"`，UI 顶部提示条展示，同时保留上次 availability，允许用户手动 Refresh。
- 用户编辑/新增/删除 server 走 `onUpdate` 更新配置；配置保存成功后，`servers` 变化会自动触发 `useEffect` 重新拉一次状态，实现"编辑即诊断"。

## CLI / Web / Admin API

### Admin API

- `GET /_bifrost/api/im-gateway/agent/mcp-status`
  - 权限：与其它 `/agent/*` 只读接口一致（Admin Web 会话或 CSRF token）。
  - 输入：无 query。
  - 输出：`{ "servers": [...] }`。
  - 副作用：短暂启动子进程/连接后立即释放。
- `PATCH /_bifrost/api/im-gateway/agent/config`
  - 保持既有语义，用于 UI 保存 `mcp_servers` 字段（`agent_api.rs:1746-1755`）。

### Web

- 入口：`/_bifrost/settings?tab=agent&agentSection=mcp-servers`
- 交互：
  - 卡片列表：显示 server 名、类型（stdio/http/sse）、启用开关、Edit/Delete、状态 tag。
  - `Refresh Status` 按钮：调用 `refreshAvailability`。
  - 状态区：Loading / Available (`Tools: N`) / Unavailable (`error`) / Disabled / Not checked。

### CLI

本节能力主要面向 Agent Settings UI，不新增外部 CLI 子命令。用户如需脚本化，可直接调用 Admin API：

```bash
curl -H "Cookie: <bifrost-admin-session>" \
     -H "X-CSRF-Token: <token>" \
     http://127.0.0.1:<admin-port>/_bifrost/api/im-gateway/agent/mcp-status
```

## Sync 边界

- MCP Server 配置 (`agent_config.mcp_servers`) 走既有 Agent config 存储；本次不改 sync 语义。
- `/agent/mcp-status` 是纯只读探测接口，**不入库、不广播、不同步**到其它设备。每台设备的检查结果只反映本机 runtime 实际能否拉起，不同设备（不同 PATH、不同 CA、不同 proxy）结论可能不同，属正确行为。
- 状态结果只保留在前端 React state，页面刷新即重算，不缓存到 localStorage。

## Phase 1-4

### Phase 1：后端 runtime 与探测函数

1. 定义 `McpServerAvailability` / `McpServerAvailabilityStatus`。
2. 提炼 `start_one_server`，让状态检查与真实 Agent runtime 共享启动逻辑。
3. 实现 `check_server_availability` / `check_server_availability_with_http_network`，含 disabled 跳过、并发 buffer_unordered、启动超时、按 name 排序。
4. 单测 `test_mcp_availability_disabled_server_is_not_started`、`test_mcp_availability_reports_invalid_enabled_server`。

### Phase 2：Admin API

1. 在 `im_gateway/agent_api.rs` 增加 `if rest == "/mcp-status"` 分支，只允许 `GET`。
2. 读取 `agent_client.model_proxy_url()`，装配 `McpHttpNetwork`，调用 runtime。
3. 单测 `agent_mcp_status_returns_servers_field`、`agent_mcp_status_method_not_allowed_for_post`。

### Phase 3：前端 UI

1. `McpServersSection.tsx` 引入 `availability` state 与 `refreshAvailability`。
2. 首次挂载 + servers key 集合变化时 auto refresh。
3. 三态 tag、`Tools: N`、错误文案、Refresh Status 按钮。
4. `checking` loading tag，避免和旧结果闪烁。
5. 加入 `data-testid` 便于 Playwright 定位：`settings-agent-mcp-status-<name>`。

### Phase 4：端到端与真实场景

1. Playwright UI spec `web/tests/ui/agent-mcp-servers.spec.ts` 覆盖 available/unavailable/disabled/refresh/主题四态。
2. `human_tests/mcp-servers.md` 三个 TC 保持一致。
3. 复查 dark theme token 可读性。

## 测试方案

### 单元测试

| 位置 | 用例 | 断言 |
| --- | --- | --- |
| `crates/agent/src/mcp/mod_tests.rs:370` | `test_mcp_availability_disabled_server_is_not_started` | disabled server 返回 `Disabled`、`tool_count=0`、不启动子进程 |
| `crates/agent/src/mcp/mod_tests.rs:389` | `test_mcp_availability_reports_invalid_enabled_server` | enabled 但命令非法的 server 返回 `Unavailable` 且带 `error` |
| `crates/agent/src/mcp/mod_tests.rs:408` | `test_mcp_default_timeout_constants_match_runtime_policy` | 检查复用的启动超时常量与 runtime 保持一致 |

### 后端集成测试

| 位置 | 用例 | 断言 |
| --- | --- | --- |
| `crates/bifrost-admin/src/handlers/im_gateway/agent_api.rs:4342` | `agent_mcp_status_returns_servers_field` | `GET /agent/mcp-status` 200，body 有 `servers` 字段 |
| `crates/bifrost-admin/src/handlers/im_gateway/agent_api.rs:4353` | `agent_mcp_status_method_not_allowed_for_post` | `POST /agent/mcp-status` 返回 405 |
| `crates/bifrost-admin/src/handlers/im_gateway/agent_api.rs:4055` | `apply_agent_config_patch_replaces_mcp_servers_and_model_providers` | `PATCH /agent/config` 能覆盖 `mcp_servers` |

### E2E / UI 测试

`web/tests/ui/agent-mcp-servers.spec.ts` 通过 route mock `mcp-status` 断言：

- `TC-MCP-SERVERS-01`：进入 `agentSection=mcp-servers` 自动请求 `mcp-status`，`mcp-status` 请求次数 >= 1，`filesystem/broken/disabled` 分别 Available/Unavailable/Disabled（`spec.ts:38-90`）。
- `TC-MCP-SERVERS-02`：available server 显示 `Tools: 3`；unavailable server 显示 `command not found`；点击 `Refresh Status` 后请求次数 >= 2。
- `TC-MCP-SERVERS-03`：切换 dark theme 后 `html[data-theme="dark"]` 存在，Available/Unavailable tag 仍可见。

### 真实场景测试

- `human_tests/mcp-servers.md` 记录三个 TC 的执行证据（2026-05-12 通过）。
- 索引：`human_tests/readme.md` 追加 `mcp-servers.md` 条目。

## Review / Fix / Test 闭环

- **第 1 轮**：复核 `McpServerAvailabilityStatus` 三态定义、`agent_api.rs` 路由是否复用 runtime 启动路径、`McpServersSection.tsx` 是否在挂载时自动触发、Playwright spec 是否断言三态。运行 `cargo test -p bifrost-agent mcp::` 和 `pnpm --dir web exec playwright test tests/ui/agent-mcp-servers.spec.ts`。
- **第 2 轮**：基于最新 diff 复查文档与 `human_tests/mcp-servers.md`、`human_tests/readme.md` 索引、失败/disabled 边界、亮/暗色 tag 可读性；再跑受影响测试。
- **第 3 轮（按需）**：若发现功能缺口、测试失败或文档不同步，追加下一轮直至关闭。

## 校验要求

- 优先执行受影响 MCP 单元测试与 Admin API 集成测试：
  - `cargo test -p bifrost-agent mcp::`
  - `cargo test -p bifrost-admin agent_mcp_status`
- 再执行 WebUI E2E：`pnpm --dir web exec playwright test tests/ui/agent-mcp-servers.spec.ts`
- 通过后按 `rust-project-validate` 要求执行 fmt、clippy 与 `cargo test --workspace --all-features`。
- `scripts/ci/local-ci.sh` 仅在需要完整本地 CI 时执行；若成本过高或范围不适用，最终交付说明原因和风险。

## 风险与决策

| 风险 | 决策 |
| --- | --- |
| 状态检查启动大量 MCP 子进程，拖慢 Settings 页 | 使用 `buffer_unordered(STARTUP_CONCURRENCY)` 控制并发，启动完立即释放；不缓存到 localStorage |
| 检查路径与 Agent runtime 分叉，UI 与实际会话结论不一致 | 强制共享 `start_one_server` + `McpHttpNetwork`（proxy/CA），任何绕过视为回归 |
| MCP server 启动会消耗 OAuth token / 触发密码提示 | disabled server 完全不启动；enabled server 的登录副作用视同"用户已同意"，因为它们也会在 Agent turn 里发生 |
| 错误摘要泄露敏感路径或凭据 | `error` 字段仅拼装 runtime 错误链的短摘要，禁止直接把 stdout/stderr 全量塞回 UI |
| 不同设备探测结果不同 | 明确不做跨设备 sync；状态是本机 runtime 事实，跨机差异属正确行为 |

## 文档更新要求

- 更新 `human_tests/mcp-servers.md`（保留三个 TC 与执行记录）。
- 更新 `human_tests/readme.md` 索引。
- 本次不新增外部用户命令或配置字段，README/协议/Hook/CLI 文档无需更新。
