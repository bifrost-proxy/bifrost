> 实施状态（2026-06-16）：worker SSE 同步阶段的本地 grant_crypto 校验、stale grant 删除、CLI `is_stale_remote_grant_error` 与 stale connection 清理、单元测试（worker / CLI 双侧）以及 e2e `TC-RI-07A`/`TC-RI-07B` 均已落地。`human_tests/remote-invoke.md` 的 `TC-RI-回归-131` 用例与执行记录也已合入。下文设计与实现一致，作为实现说明保留。

## 背景

`bifrost remote conn status` 在已有保存连接的情况下可能直接失败：

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
   - 在删除前等待 500ms 并复查 `grant_crypto`，避免与并发的 `approve_pairing` 写入竞争（实现侧最终行为）
   - 仍缺失时调用 relay `delete_grant`，并清理本地残留的 `grant_crypto` / `grant_policy` / `grant_info`
   - 例外：`AuthMethod::SshPublickey` 的 grant 不在 SSE 同步阶段删除，仅打 warn 并跳过，留给后续 `ssh_connect` 流程重建（避免误删长期 SSH key 授权）
4. 同步阶段额外清理 `local_grants` 中已不在 relay active 集里的孤儿 grant（包括其 crypto / policy / info），并对 `grant_info_store` 调用 `retain_only` 收敛持久化条目。

这样 caller 下一次执行 `remote conn status` 时，会优先在授权发现阶段直接看到 grant 已失效并提示重新 `remote conn up`，而不会把请求发到远端后才报 `missing grant shared secret`。
如果 relay 仍短暂返回 reusable grant，但后续 `open_call` 返回 `403 grant_not_active` / `grant_revoked` / `grant_missing_shared_secret` / `grant_not_found` 等 stale 授权信号（含 `find_reusable_grant error code` 同类前缀），caller 也必须将该错误归一化为“授权已过期/撤销，请重新连接”的用户文案，并同步移除本地 `remote-connections.json` 中对应记录。
同时，caller 本地这条 stale 记录被同步移除后，后续 `remote conn down` 或新的 pairing / `remote conn up` 流程不会继续复用已经失效的 grant。

## 测试方案

### 单元测试

- `test_has_usable_grant_crypto_requires_matching_local_material`
  - grant 没有本地 crypto 时返回 false
  - grant 与本地 crypto 匹配时返回 true
  - grant 与本地 crypto 的 ephemeral pub 不匹配时返回 false
- `test_is_stale_remote_grant_error_detects_open_call_403`
  - `open_call failed with status 403` 且 body 包含 `grant_not_active` / `grant_revoked` / `grant_missing_shared_secret` / `grant_not_found` 时返回 true（实现侧已把 `grant_not_found` 也归类为 stale，便于复用同一清理路径）
  - 同样适用于 `open_call error code` 与 `find_reusable_grant error code` 前缀
- `test_is_stale_remote_grant_error_rejects_scope_mismatch`
  - unrelated 403（例如 `grant_scope_mismatch`）不被归类为 stale grant
- 另有 `is_stale_grant_crypto_error`（`crates/bifrost-cli/src/commands/remote.rs`）作为兜底，匹配远端 stderr 中 `missing grant shared secret` 的旧错误格式，确保未升级的 target 上仍能识别为需要重新连接

### E2E 测试

更新 `e2e-tests/tests/test_remote_invoke_e2e.sh`，新增场景：

- 建立一条可复用的 `remote conn up -> remote conn status`
- 删除 target client 数据目录下的 `admin/remote_invoke_grant_crypto.*`
- 重启 target client 触发 SSE 重连和 active grant 同步
- 断言旧 grant 被清理，caller 再次执行 `remote conn status` 时提示授权已失效/需要重新连接，而不是远端返回 `missing grant shared secret`
- 断言 caller 本地 stale connection 也被清空；如果后续还要验证 `remote conn down`，必须先重新建立一条 fresh grant，再制造 relay `grant_not_found` 场景
- 紧接着重新建立 fresh grant，验证 `TC-RI-07B` 后续连接流程仍可正常发起，避免 stale 清理把 caller identity 或新 pairing 流程破坏

### 真实场景测试（human_tests）

更新 `human_tests/remote-invoke.md`，新增回归用例：

- `TC-RI-回归-131`：client 本地 grant crypto 丢失后，旧授权会在重连时自动收敛删除
- 同一用例补充验证 caller 本地 stale connection 会被删除，`remote conn down` 回归需基于 fresh reconnect 继续执行
- 用例必须覆盖 `open_call` 阶段收到 `grant_not_active` 时的 fallback：CLI 输出包含 `expired` / `revoked` / `connect` 语义，不包含 `missing grant shared secret`

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
