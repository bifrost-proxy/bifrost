# MCP Servers 可用性状态真实场景测试

## 功能模块说明

验证 Settings -> Agent -> MCP Servers 页面进入时会自动检查所有已配置 MCP server 的可用性，并清晰展示 `Available`、`Unavailable`、`Disabled` 状态差异。

## 前置条件

1. 在仓库根目录执行命令前先运行 `source ~/.zshrc`。
2. 前端依赖已安装：`pnpm --dir web install --frozen-lockfile`。
3. 测试使用 Playwright 自动启动隔离 WebUI/backend 环境，不复用本机默认 Bifrost 数据目录。

## 测试用例列表

### TC-MCP-SERVERS-01: 进入 MCP Servers 页面自动检查所有配置项

- 操作步骤：
  1. 执行 `pnpm --dir web exec playwright test tests/ui/agent-mcp-servers.spec.ts --grep "MCP Servers"`。
  2. 测试打开 `/_bifrost/settings?tab=agent&agentSection=mcp-servers`。
  3. 页面 mock 三个已配置 server：`filesystem`、`broken`、`disabled`。
  4. 断言页面自动请求 `/_bifrost/api/im-gateway/agent/mcp-status`。
- 预期结果：
  - `mcp-status` 请求次数大于等于 1。
  - `filesystem` 显示 `Available`。
  - `broken` 显示 `Unavailable`。
  - `disabled` 显示 `Disabled`。

### TC-MCP-SERVERS-02: 可用与不可用状态展示可诊断信息

- 操作步骤：
  1. 继续执行 `tests/ui/agent-mcp-servers.spec.ts`。
  2. 查看 `filesystem` 卡片和 `broken` 卡片。
  3. 点击 `Refresh Status`。
- 预期结果：
  - `filesystem` 卡片显示 `Tools: 3`。
  - `broken` 卡片显示失败原因 `command not found`。
  - 点击 `Refresh Status` 后 `mcp-status` 请求次数大于等于 2。

### TC-MCP-SERVERS-03: 暗色主题下状态仍可读

- 操作步骤：
  1. 继续执行 `tests/ui/agent-mcp-servers.spec.ts`。
  2. 点击页面主题切换按钮进入暗色主题。
  3. 查看 `filesystem` 与 `broken` 状态。
- 预期结果：
  - `html` 元素包含 `data-theme="dark"`。
  - `filesystem` 仍显示 `Available`。
  - `broken` 仍显示 `Unavailable`。

## 清理步骤

Playwright global teardown 自动停止测试 backend/web server 并清理临时数据目录。若测试异常中断，检查并停止残留的测试端口进程。

## 执行记录

| 用例编号 | 状态 | 日期 | 证据 |
| --- | --- | --- | --- |
| TC-MCP-SERVERS-01 | 通过 | 2026-05-12 | `pnpm --dir web exec playwright test tests/ui/agent-mcp-servers.spec.ts --grep "MCP Servers"` 通过；断言进入 `agentSection=mcp-servers` 后自动请求 `mcp-status`，并展示 Available / Unavailable / Disabled。 |
| TC-MCP-SERVERS-02 | 通过 | 2026-05-12 | 同上命令通过；断言 available server 显示 `Tools: 3`、unavailable server 显示 `command not found`，点击 `Refresh Status` 后状态接口请求次数增加。 |
| TC-MCP-SERVERS-03 | 通过 | 2026-05-12 | 同上命令通过；切换暗色主题后 `data-theme="dark"`，Available 与 Unavailable 状态仍可见。 |
