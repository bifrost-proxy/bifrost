# status TUI 远程调用状态面板

## 背景

`bifrost status --tui` 是 Bifrost 面向本机运维的低频刷新终端面板，历史上包含 Overview、Rules、Traffic 三个 tab，用 `ratatui` 生命周期驱动。远程调用（Remote Invoke）能力在 2026-Q2 大幅扩展：不只是 read-only 查询，还增加了 shell 执行、file 读写、IM 网关等 grant scope，同时通过 Settings → Remote Invoke Web 页面暴露 grant/call 管理入口。

Web 面板便于人工审批，但存在两个问题：

1. 运维在 SSH/tmux 环境下并不方便打开浏览器。
2. Bifrost 用户希望在服务器上像 `htop` 一样快速看到“哪些客户端连过来、最近谁跑了什么命令、成功/失败/耗时/输出多少字节”，与 Web 页面事实一致但不阻塞终端。

本方案给 `status --tui` 增加第 4 个 tab `Remote Invoke`，直接复用现有 Admin API，做到与 Web Settings → Remote Invoke 的事实源一致。

## 用户目标验证清单

### 必须实现（已 ship）

- `crates/bifrost-cli/src/commands/status_tui.rs` 的 tabs 数组从 `[Overview, Rules, Traffic]` 扩展为 `[Overview, Rules, Traffic, Remote Invoke]`（第 1185-1189 行）。
- 面板懒加载：Remote Invoke 数据只在 `selected_tab == 3` 或强制刷新时拉取（`plan.remote_invoke` 门控，见第 476 / 642 / 673 行）。Overview 保持原低频刷新语义。
- 数据源全部复用现有 Admin API：
  - `GET /_bifrost/api/remote-invoke/status`：worker state、pending pairing 数、active call 数。
  - `GET /_bifrost/api/remote-invoke/grants`：连接/授权过来的客户端。
  - `GET /_bifrost/api/remote-invoke/calls?limit=20`：最近调用历史。
- 客户端聚合：按 `grant_id` 或 `caller_fingerprint` 把最近调用挂到客户端行，展示最新命令预览、状态、退出码、耗时、输出字节。
- 面板下半区保留 “Recent Commands” 明细列表，直接展示最近若干条 call。
- ratatui、tab 切换、`ureq` 直连 Admin API 的现有生命周期不变。
- 未启用远程调用或数据为空时优雅降级：显示 `Remote Invoke Status` 空态而不是 panic。

### 必须不破坏

- Overview、Rules、Traffic 三个 tab 的现有行为、快捷键、刷新节奏不变。
- `status --tui` 原有 `-t` 入口不变；README 无需修改。
- 现有 Remote Invoke worker、grant、call history 数据结构与 API 响应契约不变。
- 面板显示的时间戳、字节数、退出码等格式与 Web 端一致，避免运维在两处看到不同数字。

### 必须真实验证

- CLI 真实运行 `bifrost status --tui` → 右方向键切到 Remote Invoke tab → 面板出现 `Remote Invoke`、`Connected Clients`、`Recent Commands`、`State` 关键字段。
- 面板下的 latest call 数据能对应 `GET /_bifrost/api/remote-invoke/calls` 返回。
- Overview/Rules/Traffic 三个 tab 切换回去仍保持既有节奏，未产生额外后台请求。

## 产品语义

### Tab 布局

```
┌─────────────────────────────────────────────────────────────┐
│  Overview  │  Rules  │  Traffic  │  Remote Invoke           │
├─────────────────────────────────────────────────────────────┤
│  [Remote Invoke Status]                                     │
│    Worker: <state>   Pending Pairings: 1   Active Calls: 2  │
│                                                             │
│  [Connected Clients]                                        │
│    <client label>  auth=<Pair code|SSH key>  grant=<mode>   │
│      latest: <cmd> · <status> · <exit> · <duration> · <B>   │
│    ...                                                      │
│                                                             │
│  [Recent Commands]                                          │
│    <ts>  <client>  <cmd>  <status>  <exit>  <duration>      │
│    ...                                                      │
└─────────────────────────────────────────────────────────────┘
```

### 客户端展示优先级

- 优先展示人类可读名（`display_name`、`hostname`、`caller_fingerprint` 尾 6 位）。
- 授权方式标准化：`SSH key` / `Pair code` / `Grant` 等文案统一。
- 通过 `remote_invoke_labels_prefer_human_names_and_normalize_auth` 单测断言此优先级。

### 最新调用聚合

- 按 `grant_id` 首匹配 → 若 grant 已轮换则按 `caller_fingerprint` 兜底。
- 时间上取 newest first。
- 由 `remote_invoke_latest_call_matches_grant_and_prefers_newest` 单测保证。

### 状态格式

- 终态：`success/error/canceled/timeout` + `exit=<code>` + `<duration>` + `<bytes>`。
- 运行中：`running` + `<duration since call_open>`（无 exit code）。
- 由 `remote_invoke_result_formats_terminal_and_running_calls` 单测保证。

## 技术细节

### 关键位置

| 组件 | 位置 |
| --- | --- |
| Tab 结构与渲染入口 | `crates/bifrost-cli/src/commands/status_tui.rs:1185-1220` |
| Remote Invoke 面板渲染 | `status_tui.rs:1828` `render_remote_invoke(frame, area, app)` |
| 数据结构 | `status_tui.rs:326 RemoteInvokeSnapshot`、`status_tui.rs:289 remote_invoke: bool` refresh plan |
| 拉取入口 | `status_tui.rs:747 fetch_remote_invoke_snapshot(port)` |
| 刷新调度 | `status_tui.rs:476-499 plan.remote_invoke` + `status_tui.rs:642-688` async join |
| 客户端聚合 | `status_tui.rs:1948 app.remote_invoke.grants` + `1953 latest_call_for_grant` |

### 拉取节奏

- 用户切到 tab 4 时首次拉取（`needs_remote_invoke && (need_slow_refresh || force_full)`）。
- 之后继续跟随现有 slow refresh 节奏（Overview 相同的 tick）。
- 用户手动按 `r` 强制刷新时立即重取。
- 未切到 tab 4 时，Remote Invoke 数据不拉取，避免默认后台开销。

### 数据结构

```rust
struct RemoteInvokeSnapshot {
    status: Option<RemoteInvokeStatusResp>,
    grants: Vec<RemoteInvokeGrant>,
    calls: Vec<RemoteInvokeCallSummary>,
}
```

- `status.state`：worker state、pending pairings 数、active call 数。
- `grants[i]`：客户端 grant，含 `display_name`、`caller_fingerprint`、`auth_method`、`grant_mode`、`expires_at`、`last_seen`。
- `calls[i]`：`call_id`、`grant_id`、`caller_fingerprint`、`command_kind`、`command_preview`、`state`、`exit_code`、`duration_ms`、`stdout_bytes` 等。

### CLI / Web / Admin API

- CLI：`bifrost status --tui` / `bifrost status -t`；无新参数。
- Web：Settings → Remote Invoke 页面继续做审批与撤销；TUI 面板只读。
- Admin API：
  - `GET /_bifrost/api/remote-invoke/status`
  - `GET /_bifrost/api/remote-invoke/grants`
  - `GET /_bifrost/api/remote-invoke/calls?limit=20`
  - 未来若新增 `/calls?since=<ts>` 或 SSE 拉活跃流，TUI 可以在此复用；本 phase 保持轮询即可。

### Sync 边界

- Remote Invoke 相关数据不参与规则或 Values Sync；grant 与 call 记录仅本机可见。
- TUI 面板同样本机 loopback 访问 Admin API，遵循 admin auth（有 admin token 时携带）。

## Phase 1 – Tab 与数据管道

- 扩展 tab 数组、渲染分派、`RemoteInvokeSnapshot`。
- `plan.remote_invoke` 门控 + 异步 fetch join。

## Phase 2 – 聚合与格式化

- `latest_call_for_grant`：按 `grant_id`/`caller_fingerprint` 匹配 + newest first。
- 状态格式化（终态/运行中）、labels 优先级、auth method 归一化。

## Phase 3 – 渲染与空态

- 上区 `Remote Invoke Status` 三段（worker/pending/active）。
- 中区 `Connected Clients` 列表带 latest call 行。
- 下区 `Recent Commands` 明细。
- 空态渐降（无 grant 时显示 “No connected clients”）。

## Phase 4 – 文档与真实场景

- 更新 `human_tests/cli-start-stop-status.md` 与 `human_tests/readme.md`。
- README/CLI help 不改（`status -t` 入口未变）。

## 测试方案

### 单元测试（已存在，`crates/bifrost-cli/src/commands/status_tui.rs`）

- `remote_invoke_latest_call_matches_grant_and_prefers_newest`（第 2359 行）：验证同一客户端最新调用按时间聚合 + grant 轮换后按 caller fingerprint 兜底匹配。
- `remote_invoke_result_formats_terminal_and_running_calls`（第 2409 行）：验证成功终态与运行中调用的展示。
- `remote_invoke_labels_prefer_human_names_and_normalize_auth`（第 2431 行）：验证客户端展示名优先级和 SSH key / Pair code / Grant 展示归一。

### E2E 测试

- 扩展/新增 status TUI E2E：
  - 启动临时 Bifrost 服务（临时 `BIFROST_DATA_DIR`、非 9900 端口、`--no-system-proxy`、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`）。
  - 通过 PTY 运行 `bifrost status --tui`，发送右方向键切到 Remote Invoke tab。
  - 断言输出包含 `Remote Invoke`、`Connected Clients`、`Recent Commands`、`State` 关键词。

### 真实场景测试（human_tests/cli-start-stop-status.md）

- **TC-CSS-35 status TUI Remote Invoke tab**
  1. 使用临时 `BIFROST_DATA_DIR` 和动态端口启动 Bifrost，附 `--no-system-proxy` 与 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`。
  2. `curl http://127.0.0.1:<port>/_bifrost/api/remote-invoke/status`、`.../grants`、`.../calls?limit=20` 各调用一次确认事实源可用。
  3. PTY 运行 `bifrost status --tui`；发送 `→` 三次切到 `Remote Invoke` tab；等 1s 刷新。
  4. 断言 tab 高亮 `Remote Invoke`、面板显示 `Remote Invoke Status`、`Connected Clients`、`Recent Commands` 三段标题。
  5. 至少有一次真实 pair + call：观察 latest command、状态、退出码、耗时、字节数与 Web `Settings → Remote Invoke` 一致。
  6. 切回 Overview tab 检查其原有面板未受影响。

### 覆盖率与项目校验

- `cargo fmt --all -- --check`
- `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-cli status_tui -- --nocapture`
- `cargo test --workspace --all-features`
- `rust-project-validate`
- 本机 no-local-coverage 约定生效，不运行 `make coverage` / `coverage-unit`；依赖远端 CI 与本地 human_tests 真实执行。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核 tab 切换与 fetch 门控：Overview 是否仍是默认；切到 Remote Invoke 是否只拉一次而不是每帧拉。
- 复核 API 空返回或 timeout 的空态；未启用 Remote Invoke worker 时不 panic。
- 复核 latest call 聚合是否落到正确的 client 行；grant 轮换后 fallback 生效。
- 复测：三条 focused 单测、Overview/Rules/Traffic 三 tab 回归。

### 第 2 轮

- 复核格式化：`duration`、`bytes`、`exit code`、`state` 与 Web 端展示一致。
- 复核 human_tests 索引 (`human_tests/readme.md`) 与 `cli-start-stop-status.md` 更新。
- 复检 PTY E2E 是否稳定不 flake（tick 时间、颜色码 stripping）。
- 复测：`TC-CSS-35` 完整真实执行。

## 风险与决策点

- **懒加载 vs 常驻拉取**：当前选择懒加载（只在切到 tab 才拉取），换来 Overview 无额外后台开销；代价是首次进入 tab 时会看到 0.5-1s 的空态。可通过“tick 时后台预取一次 status 但不取 grants/calls”改善，本 phase 不做。
- **API 变更耦合**：`GET /calls?limit=20` 字段扩展需保持向后兼容，TUI 按 `serde(default)` 解析新增字段。
- **无授权入口**：TUI 面板只读；审批与撤销必须走 Web/CLI，不在 TUI 内做，避免误操作。
- **多用户/多端一致**：TUI、Web、CLI `remote list` 共享同一 Admin API，一致性由后端保证，不在 TUI 层做二次缓存。
