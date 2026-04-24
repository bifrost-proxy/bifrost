## 背景

`bifrost remote status` 在已有保存连接的情况下可能直接失败：

```text
Config error: missing grant shared secret for encrypted remote command; reconnect is required
```

根因不是 caller 没带加密上下文，而是 target client 在 SSE 重连后会从 relay 全量同步 active grants 到 `local_grants`，但不会校验本地是否还保留对应的 `grant_crypto`。一旦 client 本地的 `admin/remote_invoke_grant_crypto.json(.key)` 丢失、损坏或与当前 grant 不匹配，这条授权就会变成“relay 仍可复用、client 实际无法解密”的幽灵授权，直到真正收到 `open_call` 才在远端失败。

## 实现方案

修改 `crates/bifrost-admin/src/remote_invoke/worker.rs`：

1. 在 `run_sse_session` 的 `fetch_active_grants` 同步阶段，对每条 grant 校验本地是否存在可用的 `GrantCryptoMaterial`。
2. 只有 grant_id 命中本地 `grant_crypto`，且保存的 caller/client ephemeral pub 与 relay grant 不冲突时，才把 grant 加回 `local_grants`。
3. 对于缺失或不匹配的 grant：
   - 不同步进 `local_grants`
   - 立即调用 relay `delete_grant`
   - 清理本地残留的 `grant_crypto`

这样 caller 下一次执行 `remote status` 时，会在授权发现阶段直接看到 grant 已失效并提示重新 `remote connect`，而不会把请求发到远端后才报 `missing grant shared secret`。
同时，caller 本地这条 stale `remote-connections.json` 记录也会被同步移除，避免后续流程继续拿着已经被 relay 回收的 grant 去做 `disconnect` 或其他命令。

## 测试方案

### 单元测试

- `test_has_usable_grant_crypto_requires_matching_local_material`
  - grant 没有本地 crypto 时返回 false
  - grant 与本地 crypto 匹配时返回 true
  - grant 与本地 crypto 的 ephemeral pub 不匹配时返回 false

### E2E 测试

更新 `e2e-tests/tests/test_remote_invoke_e2e.sh`，新增场景：

- 建立一条可复用的 `remote connect -> remote status`
- 删除 target client 数据目录下的 `admin/remote_invoke_grant_crypto.*`
- 重启 target client 触发 SSE 重连和 active grant 同步
- 断言旧 grant 被清理，caller 再次执行 `remote status` 时提示授权已失效/需要重新连接，而不是远端返回 `missing grant shared secret`
- 断言 caller 本地 stale connection 也被清空；如果后续还要验证 `disconnect`，必须先重新建立一条 fresh grant，再制造 relay `grant_not_found` 场景

### 真实场景测试（human_tests）

更新 `human_tests/remote-invoke.md`，新增回归用例：

- `TC-RI-回归-131`：client 本地 grant crypto 丢失后，旧授权会在重连时自动收敛删除
- 同一用例补充验证 caller 本地 stale connection 会被删除，`disconnect` 回归需基于 fresh reconnect 继续执行

同步更新 `human_tests/readme.md`。

## 校验要求

1. 定向单元测试
2. `bash e2e-tests/tests/test_remote_invoke_e2e.sh`
3. 更新并执行 `human_tests/remote-invoke.md`
4. `cargo fmt --all -- --check`
5. `cargo clippy --workspace --all-targets --all-features -- -D warnings`
6. `cargo test --workspace --all-features`

## 文档更新要求

- 更新 `human_tests/remote-invoke.md`
- 更新 `human_tests/readme.md`
