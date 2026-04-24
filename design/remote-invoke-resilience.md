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

### E2E 测试
- 使用 e2e-test 技能验证 SSE 重连对账

### 真实场景测试
- 在 human_tests/ 创建对应文档验证
