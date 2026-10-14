# 并发清空后的抓包恢复

## 问题与修复

`TrafficDbStore::clear_with_active_ids` 按数据库锁 → recent cache 写锁顺序清空。
旧实现仍持有 cache 写锁时释放数据库锁，再通过 `compact_db` 重新取锁。
另一个 clear 或 record 可以持有数据库锁等待 cache，形成 AB-BA 死锁；
代理转发仍成功，但写入和 traffic push 停止。

修复在 cache 更新后立即释放 cache，使用原数据库连接执行 `compact_with_conn`。
清空、统计更新和压缩在同一个数据库临界区完成，所有写入仍遵循数据库 → cache 顺序。
保持现有删除谓词、活跃 ID 保留、序列高水位、database epoch 和压缩错误处理语义。
不改 TLS 依赖或系统代理行为。

前端清空状态在 API 返回前保持 pending，并拒绝重叠调用。失败返回 false、保留列表
和选择状态、显示错误，并解除 pending 以便重试。成功后更新列表；全量清空重置 push
游标并重新读数据库窗口，恢复清空后到响应返回间产生的记录。重新读窗口时保留
这次读取期间到达的 live 记录及较新的 server sequence，避免用旧快照覆盖新推送。

## 验证路由与目标

本次同时影响 Rust 后端和前端 API/状态链路，验证包括：

- 在原锁交接点检查数据库锁仍持有、cache 已释放，同时让第二个 clear 重叠执行。
- 32 个线程交错 clear 与 record，保留指定 active ID；最终 clear 后新增 5 条，
  检查事件、统计、epoch、序列和重新打开数据库后的持久化。
- 前端延迟 API、失败与重试、指定 ID 删除失败、响应期间新流量恢复。
- 真实二进制和浏览器：32 个 HTTP clear 与 8 路持续代理流量；后续 5 个请求的
  转发、API 查询、列表更新及 WebSocket push frame；按钮连点和 500 恢复。
- 受影响 crate 测试、前端单元/类型/lint、Rust fmt/clippy、coverage-changed 90%
  预检、适用仓库门禁及远端 CI。

Review/Fix/Test 第一轮重点复核锁顺序和失败恢复，第二轮重点复核持续新流量、
游标与测试隔离。每轮检查 diff 并复跑相关回归；发现问题继续修复、复测。
真实场景用例见 `human_tests/traffic-clear-concurrency.md`。

## 本次云环境验证记录

基线为 `4c056b22056c50e7dc7a75a71bfa03dd06bce45d`，使用官方 Rust 1.95.0。
初始工作区干净，在独立任务分支开发；旧代码对照放在独立 worktree。

| 检查 | 结果 |
| --- | --- |
| 旧代码确定性子进程回归 | 强制 AB-BA 顺序后 15 秒超时，符合预期失败 |
| 修复代码并发回归 | 2 项通过；含指定 ID 保留、连续写入、重开数据库、序列/epoch 与事件 |
| 前端单元测试 | 64 个文件，426 项通过 |
| 真实 Chromium UI/API/push | 开发模式和最终二进制内嵌前端各 3 项通过；32 个并发 clear、96 个持续请求、5 个后续请求以及连点/500/重试 |
| workspace 和 desktop Rust fmt | 通过 |
| workspace all-targets/all-features Clippy | 通过，使用 `-D warnings` |
| 前端强制类型检查、全量 ESLint | 通过；ESLint 有 7 个既有 warning、0 error |
| E2E capability contract、shell CI coverage guard | 通过；新增 UI 回归已注册到 critical matrix |
| 完整 coverage-changed | 3862 项通过、4 个 IM 模块失败、1 个既有 ignore，退出 101 |
| 失败套件产生的变更行覆盖率 | 官方同源 coverage-diff 算法为 100%（8/8），无未测文件；不等同于完整 preflight 通过 |
| workspace all-features 测试 | 在 admin crate 的 3 个 IM 失败处停止：3863 项通过、1 个既有 ignore；后续 crate 未执行 |
| 前端格式门禁 | 仓库未提供 Prettier；独立 Prettier 检查时新旧基线同三个文件均不合格 |
| 依赖审计 | 缺少 cargo-deny，入口返回失败；未改依赖或 rustls |
| 严格 human_tests 工具门禁 | 当前没有 Chrome DevTools MCP；真实 Chromium E2E 通过，但该 MCP 门禁未验证 |
| 远端完整 CI、全量覆盖率与 UI audit | 尚未验证；完整 UI workflow 为 `ui-e2e-full.yml`，可手动触发 |

完整 coverage-changed 的 4 个失败均在未修改的 IM 代码：

- `mock_inbound_validates_and_injects_group_message`：5 秒内事件未被消费；独立旧基线和新版本分别复跑均失败。
- `group_event_loop_busy_session_falls_back_to_agent_workdir`：仅全套插桩运行超时；非插桩 workspace 运行通过，旧基线单独运行 10.22 秒通过，修复版插桩二进制单独运行 10.23 秒通过。原因未确认，不能判定为已证实的基线失败。
- `group_event_loop_routes_concurrent_context_to_the_active_external_session`：group runner 未进入 active；旧基线单独运行也在 10.23 秒超时失败。
- `group_event_loop_uses_group_workdir_for_busy_default_and_route_paths`：busy group 事件循环超时；旧基线单独运行也在 20.27 秒超时失败。

三个失败已逐例在旧基线复现；fallback 用例未在旧基线单独运行复现。
所有这些用例所在的 IM 生产代码未变更；没有修复无关 IM 问题或弱化测试。
唯一既有 ignored 用例读取当前用户真实 CLI session stores；本任务没有启用它，
也没有新增 ignore、删除测试或弱化断言。

第一轮 Review/Fix/Test 修复了 API `success:false`、错误消息提取和测试初始化未等待的问题。
第二轮修复了同步读窗口期间 live push 被旧快照覆盖的问题，并把锁状态回归加强为
子进程中的真实死锁顺序；相关单元和 UI 用例复跑通过。最终复查再次检查
status/diff、范围、锁序、失败状态、push 游标、语义和门禁记录。

最终 `cargo build --bin bifrost --all-features` 通过；构建产物 SHA-256 为
`e7e3ff5e042edca344bfb59080011b75a891cadbc896953221b1fd0874868e73`。
本次提交未包含构建自动产生的 Cargo.lock workspace 版本同步。

远端草稿 PR 为 `https://github.com/bifrost-proxy/bifrost/pull/547`。
按父会明确的分工，本任务保持仓库既有 PAT 路径限制；远端 CI 由父会通过其会话
中的已连接 GitHub 只读工具独立核验，本任务未绕过 403 或改用其他凭据读取 CI。
