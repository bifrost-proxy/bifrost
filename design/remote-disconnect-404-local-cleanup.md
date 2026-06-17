# Remote Disconnect 404 本地清理修复

## 背景

`bifrost remote disconnect --all` 当前会先调用 relay `DELETE /v4/remote-invoke/grants/:grant_id`，只有远端删除成功时才移除本地 `remote-connections.json` 里的记录。

当 grant 已经在 relay 侧被清理、过期或被其他入口删除时，CLI 会收到 `404 grant_not_found`，但本地连接记录会保留，形成“幽灵连接”。

## 目标

- `bifrost remote disconnect --all` 在 relay 返回 `404` 时仍然清理本地连接记录
- 单个 `bifrost remote disconnect` 与 `--grant-id` 路径保持相同行为
- 保留其他非 404 错误的失败提示，避免吞掉真实网络或服务端异常

## 实现逻辑

### 1. CLI 删除 grant 结果显式分类

在 [crates/bifrost-cli/src/commands/remote.rs](../crates/bifrost-cli/src/commands/remote.rs) 中为 caller relay delete grant 增加结果分类（`DeleteGrantOutcome` 枚举与 `classify_delete_grant_failure` 辅助函数）：

- `Deleted`：relay 成功删除 grant
- `AlreadyMissing`：relay 返回 `404` 或响应 body 含 `grant_not_found`

判定逻辑见 `classify_delete_grant_failure(status, body)`：当 `status == NOT_FOUND` 或 `body.contains("grant_not_found")` 时归类为 `AlreadyMissing`，其余非 2xx 错误返回 `None`，由调用方按原错误向上抛出。

### 2. disconnect 统一按“本地收敛”处理

`handle_disconnect()` 的三条路径统一消费上述结果：

- `--grant-id` 路径：直接调用 `caller.delete_grant(gid, fingerprint)`，命中 `Deleted` 或 `AlreadyMissing` 都先做 `connections.retain(|c| c.grant_id != gid)` + `save_connections`，再按结果打印对应文案。
- `--all` 路径与默认单连接路径：通过 `revoke_all_matching_grants()` 循环 `find_reusable_grant` + `delete_grant`，将 `Deleted` 与 `AlreadyMissing` 都计为成功 revoke，不再因 404 中断遍历或保留本地记录。

只要结果是 `Deleted` 或 `AlreadyMissing`，都执行本地连接记录删除并输出成功信息；只有其他错误才保留失败并继续提示用户。

### 3. 输出语义

- `--grant-id` 路径
  - 远端删除成功：`✓ Grant <short_id> revoked.`
  - 远端已不存在：`✓ Grant <short_id> was already missing on relay; local record removed.`
- `--all` 路径：逐条打印 `✓ <short_id> (<device_name>)` 或 `✗ ... — <err>`，末尾汇总 `Revoked <deleted>/<total> connection(s).`；`AlreadyMissing` 在此路径下与 `Deleted` 共享 `✓` 文案，不再单独提示「already missing on relay」（planned, not yet shipped as of 2026-06-16）。
- 默认单连接路径：成功后打印 `✓ Disconnected from <device_name> (grant: <short_id>)`，同样不区分 `AlreadyMissing`（planned, not yet shipped as of 2026-06-16）。

这样既避免误报失败，也让用户在 `--grant-id` 路径下能明确知道本次成功依赖的是幂等清理，而不是 relay 真的删除了 grant。

## 依赖项

- 本地连接持久化：`remote-connections.json`
- Relay grant 删除接口：`DELETE /v4/remote-invoke/grants/:grant_id`

## 测试方案

### 单元测试

- `test_classify_delete_grant_failure_treats_404_as_already_missing`
  - 验证 `404 grant_not_found` 会被识别为 `AlreadyMissing`
- `test_classify_delete_grant_failure_rejects_other_errors`
  - 验证 `500` 等其他错误不会被误判为可吞掉的本地成功

### E2E 测试

[e2e-tests/tests/test_remote_invoke_e2e.sh](../e2e-tests/tests/test_remote_invoke_e2e.sh) 已新增 `TC-RI-08A` 用例：

- 先完成一轮 `remote connect` 并审批，落地一个新的 reusable grant
- 直接调用 relay 删除 grant，制造 CLI 再次 `disconnect --all` 时命中 `404 grant_not_found` 的场景
- 断言 `DISCONNECT_OUTPUT` 包含 `already missing on relay` / `revoked` / `disconnected` / `✓` 任一关键字（local relay 在不同分支下幂等返回可能不同）
- 断言 caller 本地 `remote-connections.json` 连接数变为 `0`

### 真实场景测试

[human_tests/remote-invoke.md](../human_tests/remote-invoke.md) 已收录两条相关回归用例：

- `TC-RI-回归-68`：客户端侧 DELETE grant 不存在的 grant 返回 404 的底层路由行为
- `TC-RI-回归-120`：覆盖「relay 已找不到 grant 时，`disconnect --all` 仍必须删除本地连接记录」；命令脚本与预期断言见 `remote-invoke.md` 第 3797 行附近

[human_tests/readme.md](../human_tests/readme.md) 索引中 `remote-invoke.md` 的用例计数与摘要已同步更新到包含上述回归项。

## 校验要求

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test -p bifrost-cli`
- `bash e2e-tests/tests/test_remote_invoke_e2e.sh`
- `cargo test --workspace --all-features`
- `bash scripts/ci/local-ci.sh --e2e-only shell`

## 文档更新要求

- 更新 `human_tests/remote-invoke.md`
- 更新 `human_tests/readme.md`
