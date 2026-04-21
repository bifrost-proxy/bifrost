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

在 [crates/bifrost-cli/src/commands/remote.rs](/Users/eden/work/github/bifrost/crates/bifrost-cli/src/commands/remote.rs) 中为 caller relay delete grant 增加结果分类：

- `Deleted`：relay 成功删除 grant
- `AlreadyMissing`：relay 返回 `404` 或 body 含 `grant_not_found`

### 2. disconnect 统一按“本地收敛”处理

`handle_disconnect()` 的三条路径统一消费上述结果：

- `--grant-id`
- `--all`
- 单个连接断开

只要结果是 `Deleted` 或 `AlreadyMissing`，都执行本地连接记录删除并输出成功信息；只有其他错误才保留失败并继续提示用户。

### 3. 输出语义

- 远端删除成功：沿用现有 `revoked/disconnected` 成功文案
- 远端已不存在：输出 `already missing on relay; local record removed`

这样既避免误报失败，也让用户知道本次成功依赖的是幂等清理，而不是 relay 真的删除了 grant。

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

更新 [e2e-tests/tests/test_remote_invoke_e2e.sh](/Users/eden/work/github/bifrost/e2e-tests/tests/test_remote_invoke_e2e.sh)：

- 先完成一轮 `remote connect`
- 直接调用 relay 删除 grant，制造 CLI 再次 `disconnect --all` 时命中 `404` 的场景
- 断言 CLI 输出仍为成功
- 断言 caller 本地 `remote-connections.json` 连接数变为 `0`

### 真实场景测试

更新 [human_tests/remote-invoke.md](/Users/eden/work/github/bifrost/human_tests/remote-invoke.md)：

- 新增回归用例，覆盖“relay 已找不到 grant 时，disconnect --all 仍必须删除本地连接记录”
- 执行后同步更新 [human_tests/readme.md](/Users/eden/work/github/bifrost/human_tests/readme.md) 索引

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
