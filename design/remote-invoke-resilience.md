# Remote Invoke 授权韧性改进

## 问题背景

当前 remote-invoke 的授权模型中，relay 仍承担了"授权会话真相源"的角色，导致在异常路径下出现状态分裂：

1. **SSE 重连不做双向对账**：SSE 断开期间丢失的 `grant_revoked` 事件不会被补偿，target 本地残留已失效的 grant
2. **local_grants 纯内存**：target 重启后 grant 状态全部丢失，完全依赖 relay 恢复
3. **Caller 侧无容错**：relay 返回不一致状态时 caller 直接报错退出，无自愈能力

## 改进方案

### 改进 1：SSE 重连双向对账

**现状**：`run_sse_session` 中 `fetch_active_grants` 只做"relay→target 合并"——把 relay 有的 grant 插入本地。不清理"本地有但 relay 已不存在"的 grant。

**改进**：SSE 重连时，以 relay 返回的 active grants 为权威集合，清理本地多余的过期 grant。

- 构建 relay_active_set（relay 返回的所有 active grant_id）
- 遍历 local_grants，凡不在 relay_active_set 中的 → 移除 grant + crypto + policy
- 保留 SSE 实时事件（grant_created/grant_revoked）作为增量通道

### 改进 2：local_grants 持久化

**现状**：`local_grants` 是 `Arc<RwLock<HashMap<String, GrantInfo>>>`，重启即丢。

**改进**：新增 `GrantInfoStore`，与 `GrantCryptoStore`/`GrantPolicyStore` 同模式：
- 文件：`remote_invoke_grant_info.json`
- 启动时从磁盘恢复 → 合并到 local_grants
- grant 变更时同步写盘（insert/remove）
- SSE 重连对账时批量写盘
- Grants 列表面向排障展示两个时间：
  - `first_connected_at`：等同本地 `first_authorized_at`，表示该 grant 第一次在 target 侧建立授权/连接的时间；pair-code grant 来自审批落地时间，SSH grant 来自 SSH connect 成功时间。
  - `last_command_at`：仅在 `call_open` 通过本地 grant 校验并准备执行真实远程命令时更新；单纯 SSH 连接、grant 列表刷新、SSE 对账不会更新该字段。

**展示要求**：
- Admin API `GET /_bifrost/api/remote-invoke/grants` 返回 `first_connected_at` 与 `last_command_at`，并保留 `first_authorized_at` / `last_used_at` 兼容字段。
- WebUI Settings → Remote Invoke → Grants 行内展示 `connected <time>` 与 `last command <time>`，未执行过命令时显示 `-`。
- CLI `bifrost setting grant list` 展示 `first connected` 与 `last command`，便于不用打开 UI 也能判断授权是否只是连上、还是实际执行过命令。

### 改进 3：Caller 侧容错

**现状**：`find_reusable_grant` 返回 None 时 caller 直接 `exit(1)`。

**改进**：
- 如果 relay 返回无可用 grant，自动从本地 connections 中移除该条目（标记为 stale）
- 给出更明确的错误信息和修复建议

## 测试方案

### 单元测试
- `test_grant_info_store_round_trip`: 验证写入/读取/按 relay 过滤
- `test_grant_info_store_remove`: 验证移除后不再出现
- `test_sse_reconcile_purges_stale_grants`: 验证对账逻辑
- `test_validate_grant_accepts_active_permanent`: 验证有效 grant 执行命令时同步写入 `last_command_at`
- `test_validate_grant_accepts_once_and_consumes`: 验证 once grant 消耗时也写入 `last_command_at`

### E2E 测试
- 使用 e2e-test 技能验证 SSE 重连对账
- 更新 `e2e-tests/tests/test_remote_invoke_recent_calls_args_preview_e2e.sh`：pair-code 授权后先断言 Grants API 有 `first_connected_at` 且 `last_command_at` 为空；执行一次远程搜索命令后断言 `last_command_at` 非空且不早于 `first_connected_at`。

### 真实场景测试
- 更新 `human_tests/remote-invoke.md`，新增 Grants 时间字段回归用例，并按文档逐条执行：进入 discovery、批准授权、查看 Grants、执行远程命令、再次查看 Grants。
