# Remote Invoke Stale Grant / Crypto 清理

> 实施状态（2026-06-16，2026-07-03 复核）：worker 侧 SSE 同步阶段的本地 `grant_crypto` 校验、stale grant 主动删除、CLI `is_stale_remote_grant_error` / `is_stale_grant_crypto_error`、`remote-connections.json` 中 stale connection 的自动清理、以及配套单元/E2E/human 测试均已合入 `main`。本设计文档与实现同步，作为可回溯的实现说明保留。

## 背景

`bifrost remote conn status` 与其它 caller 命令在已有保存连接的情况下可能直接失败，典型错误：

```text
Config error: missing grant shared secret for encrypted remote command; reconnect is required
```

或 relay 返回 `403 grant_not_active` / `grant_revoked` / `grant_missing_shared_secret` / `grant_not_found`。

根因并不是 caller 忘带加密上下文，而是：

- target client 每次 SSE 重连后都会从 relay 全量拉取 `fetch_active_grants` 并同步到本地 `local_grants`；
- 但同步阶段没有校验本地是否仍保留可用的 `GrantCryptoMaterial`（存放在 `admin/remote_invoke_grant_crypto.json(.key)`）；
- 一旦 client 数据目录被人为清理、备份还原、跨机器迁移或磁盘损坏导致 crypto 材料丢失，那条 grant 就变成 “relay 视为 active、client 无法解密” 的幽灵授权；
- caller 拿着旧 `remote-connections.json` 走 lookup / open_call 时，要么 target 在解密阶段直接失败，要么 relay 直接返回 stale grant 错误，用户看到的只是难以理解的 500 / 403，没有任何 “请重连” 的引导。

同时，caller 端在 stale 场景下并不会主动清理 `remote-connections.json` 中的旧记录，导致下一次 `remote conn up` / `remote conn down` 仍旧尝试复用这条已经死亡的授权，形成循环失败。

## 用户目标验证清单

### 必须实现

- target 端 SSE 重连时，只把本地 crypto 完整、且 ephemeral 匹配的 grant 加入 `local_grants`；缺失或不匹配的 grant 从 relay 主动删除。
- SSH publickey 授权在 SSE 同步阶段不删除，仅打 warning，避免长期 SSH key 授权被 crypto 目录短暂缺失误删。
- 同步阶段清理 `local_grants` 中已不在 relay active 集里的孤儿 grant，并对 `grant_info_store` 调用 `retain_only`。
- caller 收到 `grant_not_active` / `grant_revoked` / `grant_missing_shared_secret` / `grant_not_found` 时归一化为 “授权已过期/撤销，请重新连接” 的稳定文案。
- caller 收到 stale 错误后，主动从 `remote-connections.json` 中移除对应连接。
- caller 收到 target stderr 中旧格式 `missing grant shared secret` 文案时也走同一 stale 分支（兼容未升级的 target）。

### 必须不破坏

- SSH publickey 授权在正常场景下继续生效；只有 `ssh_connect` 明确失败或人工 revoke 才会被删除。
- 正常授权流程（pair code / SSH key）继续可以复用 `remote conn up` → `remote conn status` → `remote exec` 全链路。
- 单次瞬时错误（例如 relay 短暂 5xx、网络抖动）不会被误当成 stale grant 清理。
- caller ephemeral 私钥、caller 长期身份不会因 stale 清理而丢失。

### 必须真实验证

- CLI 层：手工删除 target 数据目录下的 `admin/remote_invoke_grant_crypto.*` 后，caller 再次 `remote conn status` 提示重连而不是回退到 500。
- worker 侧单元测试覆盖 crypto 匹配 / 不匹配 / 缺失三种情况。
- CLI 侧单元测试覆盖三个 stale 错误码前缀（`open_call`、`find_reusable_grant`、legacy stderr）。
- E2E 脚本 `TC-RI-07A` / `TC-RI-07B` 覆盖 stale 清理后再次成功建连的完整路径。

## 产品语义

`remote-connections.json` 中的一条记录代表 “caller 与某个 client_instance 的一段可复用授权关系”，只有满足以下条件才算 usable：

1. relay 视为 active；
2. target client 本地 `grant_crypto` 完整并且和 relay grant 的 ephemeral pub 匹配；
3. caller 本地的 `caller_eph_priv` 未丢失；
4. grant 未 revoke、未 expire。

任意一条被打破，都必须被视为 stale，并给出 “重连” 引导，绝不能让 caller 在业务命令阶段才遇到不可读的错误。

## 技术细节

### target 侧 worker（`crates/bifrost-admin/src/remote_invoke/worker.rs`）

关键改造点（`run_sse_session::fetch_active_grants` 分支，实现见 `worker.rs:1346-1448` 与 `worker.rs:2163-2244`）：

1. `has_usable_grant_crypto(&transport_snapshot, &grant_info)`（`worker.rs:3912`）判定：
   - grant_id 命中 `transport.grant_crypto`；
   - `grant_crypto.caller_ephemeral_pub` 与 relay grant 一致；
   - `grant_crypto.client_ephemeral_pub` 与本地生成的 client_eph 一致。
2. 命中：加入 `local_grants`。
3. 未命中：
   - 等待 500 ms 再复查 crypto（`worker.rs:2211`），避免与并发 `approve_pairing` 写入竞争；
   - 复查仍未命中 → SSH publickey grant：warn + skip（`worker.rs:2219`）；其他 grant：调用 `relay_client.delete_grant(grant_id)`（`worker.rs:1434`、`worker.rs:2234`），并清理本地 `grant_crypto` / `grant_policy` / `grant_info`。
4. 同步收尾：`grant_info_store.retain_only(active_ids)` 保证持久化视图与 relay 收敛。

### caller 侧 CLI（`crates/bifrost-cli/src/commands/remote.rs`）

- `is_stale_remote_grant_error`（`remote.rs:1356`）判定 `BifrostError` 是否包含 `open_call failed with status 403` 且 body 含 `grant_not_active` / `grant_revoked` / `grant_missing_shared_secret` / `grant_not_found` 中任一，或 `open_call error code` / `find_reusable_grant error code` 前缀。
- `is_stale_grant_crypto_error`（`remote.rs:1375`）用于 legacy target：匹配 `CallResult.stderr` 中 `missing grant shared secret` 字符串。
- 命令主循环（`remote.rs:2156` 附近）在捕获 stale 错误后：
  - 从 `CONNECTIONS_FILE = "remote-connections.json"`（`remote.rs:51`）中移除对应记录；
  - 输出 “Grant expired/revoked, please run `bifrost remote conn up` again” 类稳定文案（不含 legacy `missing grant shared secret` 字样）；
  - 不再向 relay 复用该 grant，也不会让 `remote conn down` 走已经死亡的记录。

### relay / sync-server

无 schema 改动，复用现有 `DELETE /v5/remote-invoke/grants/:id` 端点。

## CLI 行为矩阵

| 场景 | 用户命令 | 观察结果 |
|------|----------|----------|
| target crypto 丢失（非 SSH） | `bifrost remote conn status` | 输出 “授权已过期/撤销”，`remote-connections.json` 对应记录被删除 |
| target crypto 丢失（SSH publickey） | 同上 | target 侧 warn 不删除；caller 侧若 relay 仍活可继续；否则同上 |
| relay 主动 revoke 或超期 | `bifrost remote exec ...` | 归一化文案 + 本地记录清理 |
| legacy target 返回 `missing grant shared secret` | `bifrost remote exec ...` | CLI 依旧识别为 stale 并清理 |
| 短暂 500 / 网络抖动 | 任何命令 | 不会误清理，正常报错 + 重试指引 |

## Admin API 与 Web

本次改动不新增 admin API 或 UI，Web `RemoteInvokeTab` 中 grants 列表由 `retain_only` 与 relay revoke 事件驱动，stale grant 收敛后会自动从列表消失。

## Sync 边界

- Sync 仅通过 relay 广播 grant / crypto 事件，不额外持久化 caller 端的 stale 状态。
- `remote-connections.json` 是 caller 本地文件，不参与 sync；stale 清理只影响本机。
- SSH key 授权在 sync 层不做补偿；worker 端只做 warn，避免被误当作 stale。

## Phase 拆分

- Phase 1：worker 侧引入 `has_usable_grant_crypto` 与主动 `delete_grant` 分支，加入 500 ms 复查窗口。
- Phase 2：CLI 引入 `is_stale_remote_grant_error` / `is_stale_grant_crypto_error` 与 `remote-connections.json` 清理，落归一化文案。
- Phase 3：E2E 与 human_tests 更新，把 stale 清理路径纳入回归。
- Phase 4：文档与 troubleshooting 更新，明确 “重连” 是唯一恢复路径。

## 测试方案

### 单元测试

`crates/bifrost-admin/src/remote_invoke/worker.rs`：

- `test_has_usable_grant_crypto_requires_matching_local_material`（`worker.rs:5534`）
  - grant 无本地 crypto → false
  - grant 与本地 crypto 完整匹配 → true
  - grant 与本地 crypto ephemeral pub 不匹配 → false

`crates/bifrost-cli/src/commands/remote.rs`：

- `test_is_stale_remote_grant_error_detects_open_call_403`（`remote.rs:8895`）
  - `open_call failed with status 403` 且 body 含四种 stale 关键字之一 → true
  - `open_call error code` / `find_reusable_grant error code` 前缀同样命中
- `test_is_stale_remote_grant_error_rejects_scope_mismatch`（`remote.rs:8921`）
  - `grant_scope_mismatch` 等无关 403 → false
- CLI 输出断言：`assert!(!message.contains("missing grant shared secret"))`（`remote.rs:8989`）保证归一化文案不再泄漏 legacy 错误。

### E2E 测试

`e2e-tests/tests/test_remote_invoke_e2e.sh`：新增/扩展 `TC-RI-07A` 与 `TC-RI-07B` 场景：

- 建立 fresh `remote conn up -> remote conn status`；
- 删除 target 数据目录下 `admin/remote_invoke_grant_crypto.*`；
- 重启 target 触发 SSE 重连；
- 断言 relay 中旧 grant 被删除，caller 侧 `remote conn status` 输出重连提示；
- 断言 caller 本地 stale connection 从 `remote-connections.json` 移除；
- 后续需要验证 `remote conn down` 时，先再走一次 fresh pairing，再通过 relay 制造 `grant_not_found` 场景；
- 紧接着重新建立 fresh grant，验证 `TC-RI-07B` 后续连接流程仍可正常发起，避免 stale 清理把 caller identity 或新 pairing 流程破坏。

启动参数固定 `--no-system-proxy`、临时 `BIFROST_DATA_DIR`、非 9900 端口、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1`。

### human_tests

`human_tests/remote-invoke.md`：

- `TC-RI-回归-131`：client 本地 grant crypto 丢失 → SSE 重连自动清理旧授权。
- 用例补充：caller 本地 stale connection 会被删除，`remote conn down` 回归需要基于 fresh reconnect 继续执行。
- 用例必须覆盖 `open_call` 阶段收到 `grant_not_active` 的 fallback：CLI 输出包含 `expired` / `revoked` / `connect` 语义，不再包含 `missing grant shared secret`。

同步刷新 `human_tests/readme.md` 中用例数。

### 校验命令

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test -p bifrost-admin -p bifrost-cli`
- `bash e2e-tests/tests/test_remote_invoke_e2e.sh`
- 按 `human_tests/remote-invoke.md` 手工回归 `TC-RI-回归-131`

本机遵循 no-local-coverage 约定，交付时说明 coverage 本地豁免，依赖 CI 与真实回归。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核用户目标：crypto 缺失 → stale 清理 → 重连提示 → 本地记录清理链路完整。
- 复核 diff：worker / CLI / e2e / human_tests 是否成套；SSH publickey 是否被误删。
- 复测：worker 单元测试、CLI 单元测试、`test_remote_invoke_e2e.sh` 中 `TC-RI-07A/B`。

### 第 2 轮

- 复核第 1 轮问题的修复。
- 检查 `git status --short`、`git diff`；确认没有把 caller identity / 新 pairing 流程改坏。
- 复测 stale 场景 + 立即 fresh reconnect 场景；跑 `bash e2e-tests/tests/test_remote_invoke_e2e.sh` 完整脚本。

## 风险与决策

- SSH publickey 授权保留策略：只 warn 不 delete，避免长期授权因 crypto 目录短暂缺失被清理；代价是 SSH 通路的 stale 需要通过 relay 侧 `revoke` 显式清理。
- 500 ms 复查窗口：与 `approve_pairing` 并发写入的经验值，不追求严格串行；后续如出现竞争，可以下沉到 pairing 写入完成通知。
- CLI 归一化文案：目前使用固定字符串以便自动化断言，未来接入 i18n 时需要与 e2e 用例一起改。
- `find_reusable_grant error code` 前缀识别：属于 tolerant fallback，若 relay 后续新增更多前缀需同步扩展白名单。
