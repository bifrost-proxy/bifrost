# Remote Invoke v5 PoP Hardening（P0 修复方案）

> 实施状态（2026-07-03 复核）：本稿覆盖第二轮安全复审的 P0-1 / P0-2 / P0-3 / P0-4 修复方案。相关服务侧改造已合入 `packages/bifrost-sync-server/src/remote-invoke/{service.ts,ssh-auth.ts}` 与 `packages/bifrost-sync-server/src/routes/remote-invoke.ts`，vitest 用例落在 `packages/bifrost-sync-server/src/__tests__/p0-hardening.test.ts`（P0-1 / P0-2 / P0-3 / P0-4 四段 describe 均已启用），PPE 全量回归脚本落在 `e2e-tests/tests/test_remote_invoke_ppe_full_e2e.sh`，caller 侧 CLI 已同步支持 claim_token 兑换 grant_session_token 与 PPE header 转发。

## 背景

v5 PoP 主稿把 caller 长期身份 (Ed25519 SPKI) 与每次请求的 PoP envelope 绑定，试图收敛之前 “grant_id 满天飞” 的攻击面。第二轮安全复审在真实源码上又扒出四条 P0：

- **P0-1**：`ssh-auth.ts` 的 `routeByDeviceCode` / `deviceCodeByClientInstanceId` 全是全局命名空间，没有 user 维度；`service.ts` 的 `registerClient` / `clientHeartbeat` 直接把 client 上报的 `ssh_device_route` 转发给 `syncSshRoute`，任意 user 都能占用别人的 device_code。
- **P0-2**：`lookupGrantSession` 允许 caller 每次带一把新的 `caller_ephemeral_pub` 静默覆盖，等于免 client 二次同意接管 ECDH 会话。
- **P0-3**：`submitSshConnectResult` 的 approved 分支直接落库 `permanent` + `max_calls=999999` + `user_id=''`，完全绕过 v5 claim_token / grant_session_token 链路。
- **P0-4**：`startPairing` 把 caller 上报的 `caller_info.fingerprint` 直接写进 pairing 记录并推给 client UI，攻击者可以伪装为合法 caller 骗取用户批准。

2026-06-29 追加：PPE 环境（`bifrost.example.com`）出现真实双 Bifrost 场景下 `POST /v5/remote-invoke/pairings/start` 后端 404 的回归。根因是 TLB 在 `/v5/` 前缀被剥离后转发为 `/remote-invoke/*`。此外 caller / target 都要在 PPE 上带上 `x-tt-env=ppe_ticket_system` 与 `x-use-ppe=1` header，用于发布前真实环境验证。

## 用户目标验证清单

### 必须实现

- SSH 路由必须绑定 (device_code, user_id)。跨 user 抢占抛 `device_code_owned_by_other_user`。
- SSH approval 不再直接颁发 grant_session_token；改为下发一次性 `claim_token`，caller 必须走 `POST /v5/remote-invoke/grants/claim` 带 PoP envelope 兑换。
- SSH grant `grant_mode` 强制夹紧到 `1d`（`clampSshGrantMode`），`max_calls` 走 policy 默认（1000），`user_id` 不允许空。
- `lookupGrantSession` 冻结 `caller_ephemeral_pub`：非法轮换抛 `ephemeral_pub_rotation_not_allowed`，legacy 空值允许首次绑定。
- 显式 ephemeral 轮换必须走独立端点，caller + client 双方 UI 都参与同意（见 hardening 稿 §2）。
- `startPairing` 必须从 caller envelope 中拿 `caller_pubkey`，server 端调用 `ed25519FingerprintFromBase64` 派生 fingerprint，覆盖 caller 上报值；缺 caller_pubkey 抛 `caller_pubkey_required_for_pairing`。
- `redeemClaim` PoP envelope 的 fp 必须与 pairing.caller_pubkey 派生 fp 一致，不一致抛 `caller_pubkey_mismatch`；legacy 空值同样拒绝。
- server 入口把 TLB 剥掉 `/v5` 前缀后的 `/remote-invoke/*` 归一化到 `/v5/remote-invoke/*`。
- 移除 `/v4/remote-invoke/pairings/start`、`watch`、`grants/reusable`、`calls/open` 等旧 caller 敏感入口的路由注册。
- 引入进程级 `BIFROST_REMOTE_RELAY_HEADERS` 环境变量，target 与 caller 两侧统一转发；拒绝 `authorization` / `cookie` / `host` / `x-bifrost-token` 敏感 header 覆盖。

### 必须不破坏

- 正常 pair_code + PoP 主路径（`pairings/start` → `pairings/watch` → `grants/claim` → `calls/open`）继续可用。
- 正常 SSH key 流程升级后仍可端到端建连，只是 approve 后需要 caller CLI 兑换 claim_token。
- v4 client 侧的注册 / stream 路由不被扩大暴露面。
- 现有 `remote-connections.json` 与 target 端 crypto material 结构兼容。

### 必须真实验证

- vitest 覆盖 P0-1 / P0-2 / P0-3 / P0-4 全部拒绝路径。
- E2E 覆盖 PPE / staging 真实全链路（`test_remote_invoke_ppe_full_e2e.sh`）。
- 上线后 1 周监控 `device_code_owned_by_other_user` / `ephemeral_pub_rotation_not_allowed` / `caller_pubkey_required_for_pairing` / `caller_pubkey_mismatch` 出现频次。

## 全局原则

1. 不破坏 v5 PoP 主干：所有 P0 修复在 service / route / ssh-auth 三层内部完成，不引入对外新端点（`grants/ephemeral-rotate` 归属 addendum 稿）。
2. schema 兼容：本轮不改表结构；唯一例外是 P0-3 需要新增轻量 `bifrost_remote_invoke_ssh_claims (token_hash PK, grant_id, client_instance_id, caller_pubkey_fp, expires_at)`。
3. 严格优先用 server 端可信值，caller / client 自报字段一律降级为展示 hint。
4. 回滚：每个 P0 修改独立，可单独 revert。

## 产品语义

- SSH 授权是长期 caller 的固定入口，但必须与 v5 PoP 主流程共享 claim → session token 兑换契约。
- `caller_ephemeral_pub` 属于 grant 生命周期内的 ECDH 主密钥，必须在首次绑定后冻结；任何轮换必须双向同意。
- pairing UI / SSE 事件 / 审计日志中出现的 fingerprint，都必须与后续 claim 时 PoP envelope 派生的 fingerprint 完全一致，才能形成端到端可信链路。

## 技术细节

### P0-1 + P0-3：SSH 路由绑定 user + SSH 审批下沉到 v5 claim

**RouteEntry 携带 user_id**：`ssh-auth.ts:74-120` 附近的 `RouteEntry` 与 `PendingConnectEntry` 类型都新增 `userId` 字段；`syncSshRoute(clientInstanceId, userId, route)` 在 previous route / candidate 存在但 `userId` 不匹配时抛 `device_code_owned_by_other_user`。

**调用点必须传真实 userId**：`service.ts` 的 `registerClient` 直接使用当前请求 user；`clientHeartbeat` 通过 `storage.remoteInvoke.getClientRecord(client_instance_id)` 反查 `client.user_id`，绝不再传 `''`。

**submitSshConnectResult 降级为 claim_token**：approved 分支不再直接 INSERT permanent grant，而是：

1. `clampSshGrantMode(req.grant_mode)` 把 `permanent` / `1d` 之外的值夹紧到 `1d`（`service.ts:93`、`service.ts:1166`）；
2. `caller_pubkey` 缺失 → `caller_pubkey_required`；
3. server 端派生 `caller_fingerprint = ed25519FingerprintFromBase64(caller_pubkey)`（`service.ts:1173`）；
4. INSERT grant，`user_id = pending.userId`、`grant_mode` 夹紧、`max_calls = config.ssh_grant_max_calls ?? 1000`；
5. `storage.remoteInvoke.createSshClaim({ claim_token_hash, grant_id, client_instance_id, caller_pubkey_fp, expires_at })`（`service.ts:1220`）；
6. `pushToClient('ssh_connect_complete', { connect_id, status: 'approved', grant_id, claim_token, claim_expires_at })`。

caller 拿到 claim_token 后调 `POST /v5/remote-invoke/grants/claim` 带 PoP envelope 兑换 `grant_session_token`。老 caller 收不到 grant_session_token 会失败，需要升级 CLI（BREAKING）。

### P0-2：`lookupGrantSession` 冻结 caller_ephemeral_pub

`service.ts:695` 附近：

```ts
if (
  grant.caller_ephemeral_pub &&
  req.caller_ephemeral_pub &&
  grant.caller_ephemeral_pub !== req.caller_ephemeral_pub
) {
  throw new Error('ephemeral_pub_rotation_not_allowed');
}
if (!grant.caller_ephemeral_pub && req.caller_ephemeral_pub) {
  await this.storage.remoteInvoke.updateGrantCallerEphemeralPub(
    grant.id,
    req.caller_ephemeral_pub,
  );
}
```

显式轮换路径由独立端点 `POST /v5/remote-invoke/grants/ephemeral-rotate` 承担，caller + client 双方 UI 均需 approve，写审计事件，超出本稿范围。

### P0-4：pairing 中 `caller_fingerprint` server 派生

`service.ts:377-396` 附近 `startPairing`：

- 强制读取 `req.caller_pubkey || req.caller_info?.caller_pubkey`；缺失 → `caller_pubkey_required_for_pairing`；
- `callerFingerprint = ed25519FingerprintFromBase64(callerPubkey)`；
- 把派生 fingerprint 写入 `pairing.caller_fingerprint`、`caller_info_json.fingerprint`、`pushToClient('pairing_request', ...)` 载荷中的 `caller_fingerprint` 与 `caller_info.fingerprint`；
- 存储 `pairing.caller_pubkey`，供 `redeemClaim` 校验。

`redeemClaim`（`service.ts:637-641`）：

```ts
if (pairing.caller_pubkey) {
  const expectedFp = ed25519FingerprintFromBase64(pairing.caller_pubkey);
  if (expectedFp !== callerPubkeyFp) throw new Error('caller_pubkey_mismatch');
} else {
  throw new Error('caller_pubkey_mismatch');
}
```

老 caller 没在 `caller_info` 里带 `caller_pubkey`，需要升级 CLI。

### PPE `/v5` 路由回归修复

`packages/bifrost-sync-server/src/routes/remote-invoke.ts`：

- server 入口把 `/remote-invoke/*` 归一化为 v5 caller 协议路径 `/v5/remote-invoke/*`；
- 移除 `/v4/remote-invoke/pairings/start`、`watch`、`grants/reusable`、`calls/open` 的注册；
- **不** 把无版本 `/remote-invoke/client/*` 映射到 v4 client 注册/stream 路由；
- vitest 覆盖：`POST /remote-invoke/pairings/start` 必须进入 v5 route 并返回 400 业务错误，v4 caller 入口返回 404（`p0-hardening.test.ts:197`）。

### `BIFROST_REMOTE_RELAY_HEADERS`

进程级环境变量，逗号分隔 `name=value`：

```bash
BIFROST_REMOTE_RELAY_HEADERS='x-tt-env=ppe_ticket_system,x-use-ppe=1'
```

target 端 worker 与 caller 端 CLI 的 relay 注册 / 心跳 / pair-code / SSE stream / grant / call 请求 / SSH 复用 / job polling 均复用同一组 header。

- relay SSE / watch / job polling 使用专用 direct SSE client，禁用 gzip/br/zstd/deflate 自动解压，显式 `Accept-Encoding: identity` + `Cache-Control: no-transform`；
- target SSE 重连只清理本地已过期 pairing，不再每次重连都调用 relay `cancel_pending_pairings`；
- 拒绝覆盖 `authorization` / `cookie` / `host` / `x-bifrost-token` 等敏感 header；
- 解析失败只 warn 并忽略。

## CLI 影响

`crates/bifrost-cli/src/commands/remote.rs`：

- `bifrost remote conn up --ssh-key ...` 后台监听 `ssh_connect_complete` 事件，收到 `claim_token` 后自动调 `POST /v5/remote-invoke/grants/claim` 带 PoP envelope 兑换 grant_session_token；
- `remote-connections.json` 加密保存 grant_session_token / 派生 key / expires_at；
- 遇到 `ephemeral_pub_rotation_not_allowed` / `caller_pubkey_mismatch` / `device_code_owned_by_other_user` 均归一化为 “请重新 `bifrost remote conn up`”；
- 透传 `BIFROST_REMOTE_RELAY_HEADERS`。

## Web / Admin API

- `RemoteInvokeTab` 的 pairing 审批弹窗展示 server 派生的 `caller_fingerprint`，无字段变动。
- Admin API 不新增端点。

## Sync 边界

- SSH claim / grant / ephemeral 状态均在 relay 中；本机 caller 只保存 grant_session_token 与派生 key。
- `BIFROST_REMOTE_RELAY_HEADERS` 只影响本进程，不落到 sync / config 持久化。

## Phase 拆分

- Phase 1：P0-1 + P0-3 合并落地 SSH route userId + SSH approval claim_token；建 `bifrost_remote_invoke_ssh_claims` 表 migration。
- Phase 2：P0-2 lookupGrantSession 冻结 + hardening addendum §2 显式轮换端点接入。
- Phase 3：P0-4 startPairing / redeemClaim 服务派生 fingerprint。
- Phase 4：PPE `/v5` 路由归一化 + `BIFROST_REMOTE_RELAY_HEADERS` + PPE 全量回归脚本。

## 测试方案

### vitest（`packages/bifrost-sync-server/src/__tests__/p0-hardening.test.ts`）

- `MySQL remote-invoke v5 schema reset detection`（`p0-hardening.test.ts:147`）
  - `does not reset when all v5 columns and tables are present`（`:148`）
  - `resets when legacy schema is missing v5 token columns`（`:152`）
  - `resets when removed policy columns are still present`（`:156`）
  - `resets when nonce or ssh claim tables are missing`（`:160`）
- `P0-1: SSH route is bound to the registering user`（`:166`）
  - `rejects cross-user device_code hijack with device_code_owned_by_other_user`（`:167`）
- `P0-4: pairing fingerprint is derived from server-trusted PoP key`（`:196`）
  - `accepts v5 caller routes when a TLB strips the /v5 prefix`（`:197`）
  - `rejects start_pairing payloads without caller_pubkey (server has no key to derive fp)`（`:211`）
  - `ed25519FingerprintFromBase64 is deterministic and decoupled from attacker-supplied fingerprint`（`:222`）
- `P0-3: SSH approval mints a single-use claim_token (DAO sanity)`（`:232`）
  - `SshClaim row is created/read/redeemed and cannot be reused`（`:233`）
  - `redeemed SSH claim grant_session_token can open multiple calls`（`:256`）
- `P0-2: lookupGrantSession freezes caller_ephemeral_pub (service-level)`（`:326`）
  - `throws ephemeral_pub_rotation_not_allowed when the existing ephemeral_pub differs`（`:327`）
- `P0-3: once-mode grants consume only after call budget is exhausted`（`:374`）
  - `keeps SSH once-mode grant active while remaining_calls is still positive`（`:433`）
  - `marks once-mode grant consumed when remaining_calls reaches zero`（`:464`）

配合：
- `grants-claim.test.ts` 全量用例；
- `grants-lookup.test.ts` 中 `rejects caller_ephemeral_pub rotation once frozen (P0-2)`（`grants-lookup.test.ts:98`）；
- `grants-revoke.test.ts` 中 SSH claim / ephemeral 相关子用例。

### E2E

`e2e-tests/tests/test_remote_invoke_ppe_full_e2e.sh`：

- 默认构建当前分支 `target/debug/bifrost`；`SKIP_BUILD=true` 可跳过。
- 从默认 Bifrost 数据目录读取登录 token，连接 `https://bifrost.example.com`。
- 覆盖 code 授权 / SSH key 授权 / remote traffic / remote file / remote exec / remote run / remote job / 连接清理矩阵。
- 允许通过 `BIFROST_REMOTE_RELAY_URL` / `BIFROST_REMOTE_RELAY_HEADERS` / `BIFROST_SYNC_STATE_FILE` / `BIFROST_SYNC_TOKEN` 覆盖默认环境。

其它相关 E2E：

- `test_remote_invoke_e2e.sh`（主链路）
- `test_remote_invoke_ssh_e2e.sh`（SSH claim_token 兑换）
- `test_remote_invoke_v5_session_refresh_e2e.sh`（session 刷新）
- `test_remote_invoke_missing_sync_token_log_e2e.sh`
- `test_remote_invoke_recent_calls_args_preview_e2e.sh`
- `test_remote_invoke_recent_calls_persistence_e2e.sh`

### 端到端回归矩阵

| 场景 | 修复前 | 修复后 | 备注 |
|------|--------|--------|------|
| 正常 pair_code | ✅ | ✅ | 主路径不变 |
| 正常 SSH key | ✅（直接 grant） | ✅（claim → grant） | caller CLI 必须 ≥ v5 |
| 老 v4 CLI 调 legacy caller 端点 | 410 | 404 | 路由已移除 |
| 老 v4 CLI 走 SSH 通路 | grant 直接落 | 收到 `ssh_connect_complete` 含 claim_token | BREAKING，需升级 |
| SSH 跨 user 抢占 | 静默替换 | `device_code_owned_by_other_user` | P0-1 |
| ephemeral 静默轮换 | 允许 | `ephemeral_pub_rotation_not_allowed` | P0-2 |
| caller_info.fingerprint 欺骗 | UI 弹伪造 fp | UI 弹 server 派生 fp | P0-4 |
| PPE TLB `/v5` prefix strip | 404 | v5 业务错误 | 归一化 |

### human_tests

- `human_tests/remote-invoke-v5-pop-hardening.md`：新增或扩展 P0-1 ~ P0-4 场景。
- `human_tests/remote-invoke.md`：SSH claim_token 兑换 + PPE header 转发用例。
- `human_tests/readme.md` 同步索引。

### 校验命令

- `npm --workspace bifrost-sync-server run lint`
- `npm --workspace bifrost-sync-server run test`
- `npm --workspace bifrost-sync-server run test:e2e`
- `cargo test -p bifrost-cli --features remote`
- `cargo test -p bifrost-admin --features remote_invoke`
- `bash e2e-tests/tests/test_remote_invoke_ppe_full_e2e.sh`（发布前）
- GitHub Actions `.github/workflows/ci.yml`（linux / mac / win）

本机 no-local-coverage，交付时说明 coverage 本地豁免。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核 P0-1 route owner 绑定路径是否 100% 覆盖 register + heartbeat；
- 复核 P0-3 SSH claim 表 migration + claim_token TTL；
- 复核 P0-2 lookup 冻结 + legacy 空值兼容；
- 复核 P0-4 pairing / redeemClaim fingerprint 派生；
- 复测 `p0-hardening.test.ts` 全部子用例、`grants-*.test.ts` 全部子用例。

### 第 2 轮

- 复核 route 归一化 + v4 caller 路由清理；
- 复核 `BIFROST_REMOTE_RELAY_HEADERS` 敏感 header 拒绝清单；
- 复测 PPE `test_remote_invoke_ppe_full_e2e.sh` 全链路。

### 第 3 轮

- staging 上跑 caller / target 双 Bifrost 真实端到端；
- 观察 4 类错误日志（`device_code_owned_by_other_user` 等）出现频次。

## 落地步骤（Mac 上执行）

1. `git checkout -b feat/remote-invoke-v5-pop-hardening`
2. 按 §P0-1 / P0-3 / P0-2 / P0-4 顺序改 `ssh-auth.ts` / `service.ts` / DAO `createSshClaim`；
3. 新增 SQL migration：`bifrost_remote_invoke_ssh_claims` 表；
4. 补齐 vitest / e2e 用例；
5. `npm test && cargo test` 全绿；
6. `git push origin feat/...` 开 MR，跟 CI 直到全绿；
7. Merge 前在 staging 跑一次 PPE 全链路。

## 风险与决策

- **BREAKING**：老 caller CLI 走 SSH 通路会收到 `ssh_connect_complete` 含 `claim_token`，需要 CLI ≥ 0.0.103 才能解析。建议同步出 CLI release。
- **回滚**：单独 revert 任一 P0 即可，互相无依赖。
- **监控**：上线后 1 周关注 sync-server 日志中 `device_code_owned_by_other_user` / `ephemeral_pub_rotation_not_allowed` / `caller_pubkey_required_for_pairing` / `caller_pubkey_mismatch` 出现频次。突增可能说明真实滥用或老 caller 未升级。
- **PPE header 白名单**：`BIFROST_REMOTE_RELAY_HEADERS` 只允许非敏感 header；如需扩展新 header 需要在 CLI + worker 两侧同步。
- **SSH once-mode**：`p0-hardening.test.ts:374` 一段专门覆盖 once-mode 消耗策略，若未来引入 “自动降级” 策略需要同步刷新。
