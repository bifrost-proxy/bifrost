# Remote Invoke v5 — Proof-of-Possession 安全重构

> 状态：已交付并回归 | 关联：`design/remote-invoke-security-redesign.md`、`design/remote-invoke-v5-pop-hardening.md`
>
> 目标：修复 P0-1（远程调用关键端点无身份证明）与 P0-2（敏感字段在 SSE / 响应中过度暴露）。
> 关键决策（用户已确认）：
> 1. 接受 schema 破坏式迁移；
> 2. 统一使用 Ed25519；
> 3. 引入 `grant_session_token`（`claim_token` + `session_token` 双 token 模式）；
> 4. 不做兼容期，硬切——sync server bump minor，旧客户端直接拒绝。

## 背景

Remote Invoke v4 的 caller 端点 `GET /grants/reusable`、`DELETE /grants/:id`、`POST /calls/open`、`GET /pairings/:id/watch` 仅校验 `caller_fingerprint`（自报字段）+ `grant_id`（UUID）。攻击者一旦从 SSE / 日志 / 旁路拿到 `grant_id`，无任何签名即可冒充 caller 撤销 grant、开新 call 或订阅 pairing。同时 `pairing.approved` 事件 SSE 直接广播 `grant_id` 与 `caller_ephemeral_pub` 给所有订阅者；`sse.ts::pushToClient` 用 `console.log` 打印事件元数据；`pairing watch` 单订阅者会被任意订阅者顶替。

v5 引入 Ed25519 Proof-of-Possession（PoP）：caller 不需要账号，也不需要长期 Bearer，但每一个敏感请求都必须用其 Ed25519 长期私钥对 canonical body 签名。`grant_id` 全面从对外响应 / SSE / URL 中消失，caller 侧只持有 `grant_session_token`，server 内部通过 `session_token_hash` 反查 grant。pairing 事件改为派发一次性 `claim_token`，caller 调 `/grants/claim` 换 `grant_session_token`；watch 端点用 `watch_token` 支持多订阅者。所有 token 入库均 sha256，禁止打印明文。

## 用户目标验证清单

### 必须实现

- caller 匿名，但每个敏感请求必须携带 `caller_pubkey` + `ts` + `nonce` + Ed25519 签名。
- `grant_id` 不出现在任何对外响应、SSE event、日志或 URL query 中。
- pairing approved 事件只广播一次性 `claim_token`，`claim_token` 消耗即销毁。
- caller `POST /v5/grants/claim` 后拿到 `grant_session_token`（30 分钟 sliding），后续 `open call` / `revoke` 用 Bearer + PoP。
- 多个 caller 可同时订阅同一 pairing 的 `watch_token`，watcher 之间不互相顶替。
- 所有 token 只存 sha256；`console.log` / `debug!` 不允许出现 token 明文。
- 旧 v4 客户端访问 v5 路由必须返回 `protocol_version_not_supported` 410。
- sync server 检测到旧 schema 时直接 `drop & recreate` 所有 `bifrost_remote_invoke_*` 表。

### 必须不破坏

- client（被调用端）现有 `client_auth_token` Bearer 语义、注册 / 心跳 / stream 上行链路不变。
- ssh-auth 路径（`/ssh/challenge`、`/ssh/connect`）保持已有 Ed25519 设计。
- Remote Invoke worker 上行 SSE / 事件框架不变。
- `client_auth_token` 拒绝出现在 query string，仅接受 `Authorization: Bearer …`（v5.3 硬性收敛）。

### 必须真实验证

- vitest：`packages/bifrost-sync-server/src/__tests__/pop.test.ts`、`grants-claim.test.ts`、`grants-lookup.test.ts`、`grants-revoke.test.ts`、`sse-multi-watcher.test.ts`、`remote-invoke-security.test.ts`、`remote-invoke-v5-test-utils.ts`。
- E2E：`e2e-tests/tests/test_remote_invoke_v5_session_refresh_e2e.sh` 覆盖 session token 到期后自动续 lookup。
- Rust caller mock：`crates/bifrost-cli/src/commands/remote.rs` 的测试模块。
- Human tests：`human_tests/remote-invoke-v5-pop.md`、`human_tests/remote-invoke-v5-pop-hardening.md`。

## 产品语义

### 信任模型

| 主体 | 长期凭据 | 会话凭据 | 用途 |
|---|---|---|---|
| client（被调用端） | `client_auth_token`（Bearer，sha256 入库） | — | 上行 SSE、注册、完成 connect |
| caller（调用端） | `caller_pubkey`（Ed25519，与 SSH key 同源） | `grant_session_token`（30 min sliding，sha256 入库） | 操作 grant、open call、订阅 SSE |
| 配对中 | 一次性 `pair_code` | `claim_token` + `watch_token` | 兑换 grant、订阅 pairing |

### Canonical Payload + Ed25519 PoP

所有 PoP 请求 body 顶层必含：

```jsonc
{
  "ts": 1718650000000,      // 毫秒 ts；server: |now - ts| <= 30_000
  "nonce": "32-hex",        // server 持久化去重 120s
  "caller_pubkey": "<base64 SPKI DER>",
  "signature": "<base64 ed25519 sig over canonical_json(body without 'signature')>"
  // ... 业务字段 ...
}
```

`canonical_json` 规则：递归按 key 字典序、无空格、无尾逗号、UTF-8 NFC，剔除 `signature` 字段。TS 端实现在 `packages/bifrost-sync-server/src/remote-invoke/pop.ts`；Rust 端在 `crates/bifrost-cli/src/commands/remote.rs`（`client/pop.rs` 子模块）。

## 技术细节

### 端点对照

| 旧 v4 | 新 v5 | 鉴权 | 备注 |
|---|---|---|---|
| `POST /v4/.../pairings/start` | `POST /v5/.../pairings/start` | 无 | 响应新增 `watch_token` |
| `GET  /v4/.../pairings/:id/watch?token=` | `GET  /v5/.../pairings/:id/watch?watch_token=` | `watch_token` | 多订阅者 |
| `POST /v4/.../pairings/:id/decision` | 不变 | `requireAuth`（owner 已登录） | approved 事件 payload 调整 |
| —（新） | `POST /v5/.../grants/claim` | body PoP + `pair_code` + `claim_token` | 兑换 `grant_session_token` |
| —（新） | `POST /v5/.../grants/ssh-claim` | SSH-signed body | SSH key 直发 grant |
| `GET  /v4/.../grants/reusable` | `POST /v5/.../grants/lookup` | PoP | 命中返回 `grant_session_token`，无 `grant_id` |
| `DELETE /v4/.../grants/:id` | `POST /v5/.../grants/revoke` | Bearer `grant_session_token` + PoP | server 由 token 反查 grant |
| `POST /v4/.../calls/open` | `POST /v5/.../calls/open` | Bearer `grant_session_token` + PoP | body 不含 `grant_id` |
| `GET  /v4/.../calls/:id/events` | 不变 | `relay_token` | relay_token 仍由 open call 返回 |
| `GET  /v4/.../calls/:id/stream` | 不变 | 同上 | — |

路由分发实现见 `packages/bifrost-sync-server/src/routes/remote-invoke.ts:234`：`/v5/remote-invoke/pairings/start`、`/v5/remote-invoke/pairings/:id/watch`、`/v5/remote-invoke/grants/lookup|claim|ssh-claim|revoke`、`/v5/remote-invoke/calls/open` 各自映射到 `handlePairingStartV5` / `handlePairingWatchV5`（`:928`）/ `handleGrantsLookupV5`（`:958`）/ `handleGrantsClaimV5`（`:992`）/ `handleGrantsRevokeV5`（`:1060`）/ `handleOpenCallV5`（`:1081`）。老 v4 路径统一返回 `{error:"protocol_version_not_supported"}` 410。

### `pairing.approved` 事件改造

```jsonc
{
  "type": "approved",
  "claim_token": "<32-hex>",       // 一次性，180s TTL
  "claim_expires_at": 1718650180,
  "grant_summary": {
    "scope": "remote_query",
    "mode": "reusable",
    "file_access": "read_write"
  }
}
```

不含 `grant_id`、`caller_ephemeral_pub`、`client_ephemeral_pub`、`caller_fingerprint`。caller 收到后调 `POST /v5/grants/claim`：body 含 `pair_code` + `claim_token` + `caller_pubkey` + 签名；server 比对 `claim_token_hash`，命中即销毁（`markPairingClaimed`），写入该 grant 的 `caller_pubkey` 并颁发 `grant_session_token`。

### 错误码

| code | HTTP | 含义 |
|---|---|---|
| `signature_invalid` | 401 | Ed25519 校验失败 |
| `timestamp_out_of_window` | 401 | `|now-ts| > 30_000` |
| `replay_detected` | 401 | nonce 重复 |
| `invalid_caller_pubkey` | 400 | 非 Ed25519 / 解析失败 |
| `caller_pubkey_mismatch` | 403 | PoP pubkey ≠ grant.caller_pubkey |
| `grant_session_token_invalid` | 401 | Bearer token 不存在 / 已撤销 / 过期 |
| `claim_token_invalid` | 401 | claim_token 不存在 / 已用 / 已过期 |
| `watch_token_invalid` | 401 | watch_token 不存在 / 已过期 |
| `protocol_version_not_supported` | 410 | 旧 v4 客户端访问 v5 路由 |
| `pairing_expired` | 410 | 已过期 pairing 的 decision |

### Schema DDL（破坏式：drop & recreate）

依据 `AGENTS.md L511`：协议更新时直接重建数据库。sqlite / mysql 同步变更（见 `packages/bifrost-sync-server/sql/init-sqlite.sql`、`init-mysql.sql`）：

```sql
-- bifrost_remote_invoke_grants 新增列
ALTER TABLE bifrost_remote_invoke_grants ADD COLUMN caller_pubkey            TEXT NOT NULL DEFAULT '';
ALTER TABLE bifrost_remote_invoke_grants ADD COLUMN caller_pubkey_fp         TEXT NOT NULL DEFAULT '';
ALTER TABLE bifrost_remote_invoke_grants ADD COLUMN session_token_hash       TEXT NOT NULL DEFAULT '';
ALTER TABLE bifrost_remote_invoke_grants ADD COLUMN session_token_expires_at TEXT NOT NULL DEFAULT '';
ALTER TABLE bifrost_remote_invoke_grants ADD COLUMN last_nonce_seen          TEXT NOT NULL DEFAULT '';
ALTER TABLE bifrost_remote_invoke_grants ADD COLUMN revoked_at               TEXT NOT NULL DEFAULT '';
CREATE INDEX idx_ri_grants_caller_fp ON bifrost_remote_invoke_grants(caller_pubkey_fp);
CREATE INDEX idx_ri_grants_session   ON bifrost_remote_invoke_grants(session_token_hash);

-- bifrost_remote_invoke_pairings 新增列
ALTER TABLE bifrost_remote_invoke_pairings ADD COLUMN watch_token_hash TEXT NOT NULL DEFAULT '';
ALTER TABLE bifrost_remote_invoke_pairings ADD COLUMN claim_token_hash TEXT NOT NULL DEFAULT '';
ALTER TABLE bifrost_remote_invoke_pairings ADD COLUMN claim_expires_at TEXT NOT NULL DEFAULT '';
ALTER TABLE bifrost_remote_invoke_pairings ADD COLUMN claimed_at       TEXT NOT NULL DEFAULT '';
CREATE INDEX idx_ri_pairings_claim ON bifrost_remote_invoke_pairings(claim_token_hash);
CREATE INDEX idx_ri_pairings_watch ON bifrost_remote_invoke_pairings(watch_token_hash);

-- nonces 去重表（新）
CREATE TABLE bifrost_remote_invoke_nonces (
  caller_pubkey_fp TEXT NOT NULL,
  nonce            TEXT NOT NULL,
  seen_at          TEXT NOT NULL,
  PRIMARY KEY (caller_pubkey_fp, nonce)
);
CREATE INDEX idx_ri_nonces_seen ON bifrost_remote_invoke_nonces(seen_at);
```

启动时检测到缺列即触发 `resetRemoteInvokeSchemaIfNeeded`，DROP 全部 `bifrost_remote_invoke_*` 表再按新 schema 重建。

### DAO 接口新增（`packages/bifrost-sync-server/src/dao/types.ts`）

```ts
getGrantByCallerFp(callerFp: string, clientInstanceId: string): RemoteInvokeGrantRow | null;
getGrantBySessionTokenHash(hash: string): RemoteInvokeGrantRow | null;
updateGrantCallerPubkey(grantId: string, pubkey: string, fp: string): void;
updateGrantSessionToken(grantId: string, hash: string, expiresAt: string): void;
revokeGrant(grantId: string, revokedAt: string): void;
markNonceUsed(callerFp: string, nonce: string, seenAt: string): boolean;  // 冲突返回 false
gcNonces(before: string): number;
getPairingByClaimTokenHash(hash: string): RemoteInvokePairingRow | null;
getPairingByWatchTokenHash(hash: string): RemoteInvokePairingRow | null;
setPairingClaimTokens(pairingId: string, claimHash: string, watchHash: string, claimExpiresAt: string): void;
markPairingClaimed(pairingId: string, claimedAt: string): void;
```

## CLI + Web + Admin API

### CLI 侧改造（`crates/bifrost-cli/src/commands/remote.rs`）

- caller 长期身份复用 `~/.bifrost/remote-device.key`（Ed25519 PKCS8）。首次使用时把公钥写入 `caller-identity.json` 的 `caller_pubkey_b64`。
- 新增 `client/pop.rs`：`canonical_json`（Rust 版，与 TS 一致）、`sign_envelope(body_json, key) -> envelope_json`（注入 `ts/nonce/caller_pubkey/signature`）。
- v4 → v5 迁移点：
  - `grants/reusable` GET → `POST /v5/grants/lookup`（PoP body），失败→`None`，成功→存 `grant_session_token`。
  - `DELETE grants/:id` → `POST /v5/grants/revoke`（Bearer + PoP）。
  - `POST calls/open` → `POST /v5/calls/open`（Bearer + PoP，body 去掉 `grant_id`，新增 `client_instance_id`）。
  - `pairings/:id/watch?token=` → `?watch_token=`；`watch_token` 由 `/v5/pairings/start` 返回。
- caller 本地缓存 `~/.bifrost/remote-connections.json`：新增 `grant_session_token`，加密存储（复用 `remote-connections.key`）；过期后自动 `lookup` 续。

### Web 层（`web/src/api/remoteInvoke.ts`）

- 全面切换 v5 URL；封装 `signRequest(body, key)` 帮助方法。
- 已存 `caller_pubkey_b64` 的用户不再看到 `grant_id`；错误码走统一 toast 表。

### Admin / Server 侧关键 handler

- `remote-invoke/service.ts::submitGrantDecision` approved 分支：生成 `claim_token = randomHex(32)`、`watch_token = randomHex(32)`；DAO `setPairingClaimTokens(...)`；watcher payload = `{type, claim_token, claim_expires_at, grant_summary}`。
- 新增：`mintClaimToken`、`redeemClaim`、`mintGrantSessionToken`、`resolveGrantBySessionToken`、`revokeGrantByBearer`。
- `openCall` 改为 `openCall(grant: RemoteInvokeGrantRow, req: OpenCallRequestV5)`；不再读 `req.grant_id`；caller_pubkey_fp 一致性由 router 层完成。

### Relay hardening（v5.3 一次性收敛）

- `client_auth_token` 只接受 `Authorization: Bearer …`，query-string fallback 已删除，避免 30 天 token 出现在访问日志 / 代理日志 / 浏览器历史。
- `openCall` 通过 DB 条件更新消费 grant call budget：`status='active' AND remaining_calls > 0`，只有影响行数 = 1 才继续，防止并发 open 让 once grant 双开。
- caller open 限流：`600` req/min per caller+client key，保留正常 `500` opens/min 的高频负载，仍能防止 tight loop。
- SSE 卫生：`pairingWatchers: Map<string, Set<{res, pairingId, watchTokenHash}>>`；`pushToClient` 三条 `console.log` 全部删除；只保留 `debug` 级别的 event 名 + `client_instance_id`，禁止打印 payload 与 token。

## Sync 边界

- 全部 v5 路由都是 relay 独有，不进入本地 Bifrost sync 流程。
- Rust caller 端 `~/.bifrost/remote-connections.json` 仅本机存储；不同步到设备之间。
- schema 变更走 sync server 自身版本升级，不通过 rules sync 广播。

## Phase 拆分

### Phase 1：协议基线

- pop.ts + canonical_json + Ed25519 校验（含 nonce/timestamp/replay 单元覆盖）。
- Schema DDL + DAO 接口 + `resetRemoteInvokeSchemaIfNeeded` 分支。

### Phase 2：Server 端点切换

- 路由分发 v5，v4 返回 410。
- `submitGrantDecision`、`openCall`、`grants/*` handler 迁移。
- SSE 多订阅者 + 日志卫生。

### Phase 3：Rust CLI + Web

- CLI grants/pairings/calls 全部切 v5；caller-identity + session token 缓存。
- Web API/UI 切 v5，错误码 toast。

### Phase 4：Relay hardening（v5.3）

- Bearer-only client token、grant budget CAS、caller 限流。
- E2E `test_remote_invoke_ppe_full_e2e.sh` 一键回归。

## 测试方案

### vitest 单元（`packages/bifrost-sync-server/src/__tests__/`）

| 文件 | 关键用例 |
|---|---|
| `pop.test.ts` | 合法签名通过 / 篡改字段失败 / nonce 重放失败 / ts 超窗失败 / 非 ed25519 失败 / canonical 排序稳定 / 剔除 signature 字段 |
| `grants-claim.test.ts` | approved 后 claim 成功 + grant 绑定 caller_pubkey / 重复 claim 失败 / 过期 claim_token 失败 / 错配 caller_pubkey 失败 |
| `grants-lookup.test.ts` | active grant → 颁发 token / 无 grant → 404 / 撤销后查不到 / 不同 caller_pubkey 隔离 |
| `grants-revoke.test.ts` | Bearer + PoP 通过 → revoke / Bearer 错 → 401 / 跨 caller 不能 revoke |
| `sse-multi-watcher.test.ts` | 同 pairing 多 watcher 都能收到 approved / watch_token 错误 → 401 |
| `remote-invoke-security.test.ts` | v4 路径返回 410；v5 缺签名返回 401 |
| `remote-invoke-relay-v2-phase1.test.ts` | open call 不再接受 `grant_id` 字段 |
| `p0-hardening.test.ts` | client_auth_token 不再接受 query；grant budget CAS 单次成功；限流 600/min |

### E2E

- `e2e-tests/tests/test_remote_invoke_v5_session_refresh_e2e.sh`：session token 到期 → 自动 `lookup` 续 → 继续 exec。
- `e2e-tests/tests/test_remote_invoke_ppe_full_e2e.sh`：一键 release 回归，含本地 sync-server security/relay/PoP vitest、Rust CLI remote unit-test filter、`bifrost-server-v4` hardening 套件、部署 relay Code + SSH key 端到端矩阵。矩阵内含普通 shell / file / traffic / job 命令的 Code 与 SSH key 授权、独立 Code `remote_power_mgmt` grant 的 `remote keep-awake`、SSH key 默认 Full Trust `remote keep-awake`。
- PPE header 注入仅通过环境变量 `BIFROST_REMOTE_RELAY_HEADERS='x-tt-env=ppe_ticket_system,x-use-ppe=1'`，不引入 UI / 持久化配置。
- `crates/bifrost-e2e/src/tests/remote_shell_exec.rs`：升级到 v5。

### human_tests

`human_tests/remote-invoke-v5-pop.md`（+ 索引 `human_tests/readme.md`）：

- TC-V5-01：pair → 桌面端审批 → CLI 自动 claim 拿 token → exec 成功。
- TC-V5-02：偷到旧 grant_id 直接 curl `/v5/calls/open` → 401。
- TC-V5-03：服务端日志 `grep -E 'token|secret|claim_token|grant_session'` → 仅出现 sha256 哈希或字面 `<redacted>`。
- TC-V5-04：同 pairing 两个 watcher 都收到 approved。
- TC-V5-05：revoke 后立刻 open call → 401。
- TC-V5-06：旧 v0.0.127 CLI 连新 sync server → `protocol_version_not_supported`。

`human_tests/remote-invoke-v5-pop-hardening.md` 追加 v5.3 相关 case：

- TC-V5-Hardening-01：client_auth_token 只走 Header，query 拒绝。
- TC-V5-Hardening-02：once grant 高并发 open call 仅一次成功。
- TC-V5-Hardening-03：limiter tight loop 命中 600/min 阈值。

### 覆盖率

- `pnpm -C packages/bifrost-sync-server coverage`：`pop.ts`、`grants-*.ts`、`sse` 多订阅者分支全覆盖，目标 ≥ 90%。
- Rust 侧 `bash scripts/ci/coverage-all.sh -p bifrost-cli --json --gate` 通过。
- 收尾 `make coverage`。

## Review/Fix/Test 闭环

### 第 1 轮

- 复核 v4 → v5 端点是否全部覆盖，无遗漏。
- 复核 SSE payload、日志、URL、错误提示中不再出现 `grant_id` / caller ephemeral pub。
- 复测：pop.test.ts、grants-*.test.ts、sse-multi-watcher.test.ts、v5_session_refresh E2E。

### 第 2 轮

- 复核 caller 长期私钥缓存路径与权限（0600）。
- 复核 schema 检测 & drop-recreate 分支是否幂等。
- 复测：ppe_full_e2e 全套；`make coverage` 关键文件 ≥ 90%。

## 风险与决策

| 风险 | 缓解 |
|---|---|
| caller 私钥泄露 | 文档建议 ssh-agent / OS keychain 托管；revoke key 一键失效 |
| `grant_session_token` 泄露 | 30 min 短 TTL + 任意 revoke 即失效 |
| nonce 表膨胀 | 周期任务清理 `seen_at < now - 120s` |
| watch_token 泄露 | pairing 生命周期 ≤ 10 min；claim 后 watcher 自动 unregister |
| schema 强制重建对未升级客户端影响 | 版本 bump minor，部署窗口停老 server → drop 表 → 升 server → 升客户端，不允许灰度并行 |

## 实施清单

1. Schema + DAO + types 同步。
2. pop.ts 工具 + 单测。
3. service.ts handler 改造 + 单测。
4. routes/remote-invoke.ts 路由切换 + 单测。
5. sse.ts 多订阅者 + 日志卫生。
6. Rust caller 端 commands/remote.rs + mock test。
7. E2E `test_remote_invoke_v5_session_refresh_e2e.sh`、`test_remote_invoke_ppe_full_e2e.sh`。
8. human_tests 新文档 + readme 索引。
9. `pnpm lint/typecheck/test/test:e2e/coverage` 全绿；`make coverage` 全绿。
10. 两轮 Review/Fix/Test；commit + push + MR + 远端 CI 看护到全绿。
