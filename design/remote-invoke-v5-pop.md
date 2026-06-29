# Remote Invoke v5 — Proof-of-Possession 安全重构

> 状态：设计草案 | 关联：design/remote-invoke-security-redesign.md
>
> 目标：修复 P0-1（远程调用关键端点无身份证明）与 P0-2（敏感字段在 SSE/响应中过度暴露）。
> 决策（用户已确认）：
> 1. 接受 schema 破坏式迁移；
> 2. 统一使用 Ed25519；
> 3. 引入 `grant_session_token`（claim_token 双 token 模式）；
> 4. **不做兼容期，硬切**——sync server bump minor，旧客户端直接拒绝。

---

## 一、问题与目标

### 1.1 现状漏洞（详见 `audit/remote-invoke-relay-security.md`）

- **P0-1**：caller 端点 `GET /grants/reusable`、`DELETE /grants/:id`、`POST /calls/open`、`GET /pairings/:id/watch` 仅校验 `caller_fingerprint`（自报字段）+ `grant_id`（UUID）。攻击者一旦从 SSE/日志/旁路拿到 `grant_id`，无任何签名即可冒充 caller。
- **P0-2**：`pairing.approved` 事件 SSE 直接广播 `grant_id` 与 `caller_ephemeral_pub` 给所有订阅者；`sse.ts pushToClient` 用 `console.log` 打印事件元数据；`pairing watch` 单订阅者会被任意订阅者顶替。

### 1.2 目标

- caller 不需要账号/不需要 token，但**每个敏感请求必须用其 Ed25519 长期私钥签名**（Proof-of-Possession）。
- `grant_id` 不再出现在任何对外响应或事件中。caller 用 `grant_session_token` 操作 grant，server 内部映射回 grant。
- `pairing approved` 事件改为派发**一次性 `claim_token`**，caller 调 `/grants/claim` 才换 `grant_session_token`；`watch` 端点改为持 `watch_token` 多订阅者。
- 所有 token 入库 sha256，禁止打印明文。

### 1.3 非目标

- 不引入 OAuth/Account 体系；caller 仍匿名。
- 不改 client（被调用端）现有 `client_auth_token` Bearer 流程。
- 不改 ssh-auth 路径（`/ssh/challenge`、`/ssh/connect`）已是 Ed25519，复用其设计风格。

---

## 二、信任模型

| 主体 | 长期凭据 | 会话凭据 | 用途 |
|---|---|---|---|
| client（被调用端） | `client_auth_token`（Bearer，sha256 入库） | — | 上行 SSE / 注册 / 完成 connect |
| caller（调用端） | `caller_pubkey`（Ed25519，与 SSH key 同源） | `grant_session_token`（30 min sliding，sha256 入库） | 操作 grant、open call、订阅 SSE |
| 配对中 | 一次性 `pair_code` | `claim_token` + `watch_token` | 兑换 grant、订阅 pairing |

---

## 三、协议规格 v5（破坏式）

### 3.1 Canonical Payload + Ed25519 PoP

所有 PoP 请求 body 顶层必含：

```jsonc
{
  "ts": 1718650000000,      // 毫秒 ts；server: |now-ts| <= 30s
  "nonce": "32-hex",        // server 持久化去重 120s
  "caller_pubkey": "<base64 SPKI DER>",
  "signature": "<base64 ed25519 sig over canonical_json(body without 'signature')>"
  // ... 业务字段 ...
}
```

`canonical_json` 规则：递归按 key 字典序、无空格、无尾逗号、UTF-8 NFC，剔除 `signature` 字段。

### 3.2 端点对照

| 旧 v4 | 新 v5 | 鉴权 | 备注 |
|---|---|---|---|
| `POST /v4/.../pairings/start` | `POST /v5/.../pairings/start` | 同前（无） | 响应新增 `watch_token` |
| `GET  /v4/.../pairings/:id/watch?token=` | `GET  /v5/.../pairings/:id/watch?watch_token=` | `watch_token` 校验 | 多订阅者 |
| `POST /v4/.../pairings/:id/decision` | 不变 | requireAuth（owner 已登录） | approved 事件 payload 调整 |
| —（新） | `POST /v5/.../grants/claim` | body PoP（`pair_code` + `caller_pubkey` + 签名） | 兑换 `grant_session_token` |
| `GET  /v4/.../grants/reusable` | `POST /v5/.../grants/lookup` | PoP | 命中返回 `grant_session_token`，无 `grant_id` |
| `DELETE /v4/.../grants/:id` | `POST /v5/.../grants/revoke` | Bearer `grant_session_token` + PoP | server 由 token 反查 grant |
| `POST /v4/.../calls/open` | `POST /v5/.../calls/open` | Bearer `grant_session_token` + PoP | body 不含 `grant_id` |
| `GET  /v4/.../calls/:id/events` | 不变 | `relay_token` 校验 | relay_token 仍由 open call 返回 |
| `GET  /v4/.../calls/:id/stream` | 不变 | 同上 | — |

### 3.3 `pairing.approved` 事件改造（关键修复）

```jsonc
// approved 事件 payload（new）
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

- 不含 `grant_id`、不含 `caller_ephemeral_pub`、不含 `client_ephemeral_pub`、不含 `caller_fingerprint`。
- caller 收到后调 `POST /v5/grants/claim`：body 含 `pair_code` + `claim_token` + `caller_pubkey` + 签名；server 比对 `claim_token_hash`，命中即销毁（`markPairingClaimed`），写入该 grant 的 `caller_pubkey` 并颁发 `grant_session_token`。

### 3.4 错误码

| code | 含义 |
|---|---|
| `signature_invalid` | Ed25519 校验失败 |
| `timestamp_out_of_window` | `|now-ts| > 30_000` |
| `replay_detected` | nonce 重复 |
| `invalid_caller_pubkey` | 非 Ed25519 / 解析失败 |
| `caller_pubkey_mismatch` | PoP pubkey ≠ grant.caller_pubkey |
| `grant_session_token_invalid` | Bearer token 不存在 / 已撤销 / 过期 |
| `claim_token_invalid` | claim_token 不存在 / 已用 / 已过期 |
| `watch_token_invalid` | watch_token 不存在 / 已过期 |
| `protocol_version_not_supported` | 旧 v4 客户端访问 v5 路由 |

### 3.5 Relay hardening follow-up

- `client_auth_token` is accepted only from `Authorization: Bearer ...` on
  client-authenticated relay endpoints. Query-string fallback is removed so the
  30-day client token cannot be copied into access logs, proxy logs, browser
  history, or monitoring URL fields.
- `openCall` consumes grant call budget through a database conditional update:
  `status='active' AND remaining_calls > 0`. The service proceeds only when
  the update affects exactly one row, so concurrent requests against a once
  grant cannot both pass a stale pre-check and create multiple calls.
- The caller open rate limiter is `600` requests/minute per caller+client key.
  This preserves a normal high-frequency workload of `500` opens/minute while
  still bounding accidental tight loops.
- `e2e-tests/tests/test_remote_invoke_ppe_full_e2e.sh` is the one-click local
  regression entry for release validation. It runs, by default, the local
  sync-server Remote Invoke security/relay/PoP Vitest suites, the Rust CLI
  remote unit-test filter, the adjacent `bifrost-server-v4` hardening suite,
  and the deployed relay Code + SSH key end-to-end matrix. The PPE request header is
  intentionally an environment-only test knob
  (`BIFROST_REMOTE_RELAY_HEADERS='x-tt-env=ppe_ticket_system,x-use-ppe=1'`);
  no UI or persistent runtime configuration is introduced for it.

---

## 四、Schema DDL（破坏式：直接 drop & recreate）

依据 `AGENTS.md` L511：协议更新时直接重建数据库，不考虑旧数据兼容。

### 4.1 sqlite / mysql 同步变更

```sql
-- bifrost_remote_invoke_grants
ALTER TABLE bifrost_remote_invoke_grants ADD COLUMN caller_pubkey            TEXT NOT NULL DEFAULT '';
ALTER TABLE bifrost_remote_invoke_grants ADD COLUMN caller_pubkey_fp         TEXT NOT NULL DEFAULT '';
ALTER TABLE bifrost_remote_invoke_grants ADD COLUMN session_token_hash       TEXT NOT NULL DEFAULT '';
ALTER TABLE bifrost_remote_invoke_grants ADD COLUMN session_token_expires_at TEXT NOT NULL DEFAULT '';
ALTER TABLE bifrost_remote_invoke_grants ADD COLUMN last_nonce_seen          TEXT NOT NULL DEFAULT '';
ALTER TABLE bifrost_remote_invoke_grants ADD COLUMN revoked_at               TEXT NOT NULL DEFAULT '';
CREATE INDEX idx_ri_grants_caller_fp ON bifrost_remote_invoke_grants(caller_pubkey_fp);
CREATE INDEX idx_ri_grants_session   ON bifrost_remote_invoke_grants(session_token_hash);

-- bifrost_remote_invoke_pairings
ALTER TABLE bifrost_remote_invoke_pairings ADD COLUMN watch_token_hash TEXT NOT NULL DEFAULT '';
ALTER TABLE bifrost_remote_invoke_pairings ADD COLUMN claim_token_hash TEXT NOT NULL DEFAULT '';
ALTER TABLE bifrost_remote_invoke_pairings ADD COLUMN claim_expires_at TEXT NOT NULL DEFAULT '';
ALTER TABLE bifrost_remote_invoke_pairings ADD COLUMN claimed_at       TEXT NOT NULL DEFAULT '';
CREATE INDEX idx_ri_pairings_claim ON bifrost_remote_invoke_pairings(claim_token_hash);
CREATE INDEX idx_ri_pairings_watch ON bifrost_remote_invoke_pairings(watch_token_hash);

-- 新增：nonces 去重表
CREATE TABLE bifrost_remote_invoke_nonces (
  caller_pubkey_fp TEXT NOT NULL,
  nonce            TEXT NOT NULL,
  seen_at          TEXT NOT NULL,
  PRIMARY KEY (caller_pubkey_fp, nonce)
);
CREATE INDEX idx_ri_nonces_seen ON bifrost_remote_invoke_nonces(seen_at);
```

服务端启动时：检测 `bifrost_remote_invoke_*` 表缺少新列时，DROP 全部 `bifrost_remote_invoke_*` 表并按新 schema 重建（已有 `resetRemoteInvokeSchemaIfNeeded` 流程，扩展之）。

### 4.2 DAO 接口（`src/dao/types.ts`）新增

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

---

## 五、源码改造清单

### 5.1 新文件：`packages/bifrost-sync-server/src/remote-invoke/pop.ts`

```ts
export interface PoPRequestEnvelope {
  ts: number;
  nonce: string;
  caller_pubkey: string;     // base64 SPKI DER
  signature: string;         // base64
  [k: string]: unknown;
}

export interface VerifyPoPOptions {
  maxSkewMs?: number;        // default 30_000
  expectedCallerPubkeyFp?: string;
}

export interface VerifyPoPResult {
  callerPubkey: string;
  callerPubkeyFp: string;
}

export function canonicalJson(value: unknown): string;
export function ed25519FingerprintFromBase64(spkiB64: string): string;
export function verifyPoP(
  body: PoPRequestEnvelope,
  opts: VerifyPoPOptions,
  markNonce: (fp: string, nonce: string, seenAt: string) => boolean,
): VerifyPoPResult;
```

校验流程：解析 `caller_pubkey` → 计算 fp → `markNonce` → 时间窗 → canonical → `crypto.verify(null, payload, key, sig)`（Ed25519，与 `ssh-auth.ts:180` 一致）→ 可选 fp 一致性。

### 5.2 `routes/remote-invoke.ts`

- 删除路由：`GET /grants/reusable`、`DELETE /grants/:id`、`POST /calls/open`、`GET /pairings/:id/watch`（保留 path 但返回 410+提示）。
- 新增路由（全部 `/v5/remote-invoke/...`）：
  - `POST /grants/claim` → `handleGrantsClaim`
  - `POST /grants/lookup` → `handleGrantsLookup`
  - `POST /grants/revoke` → `handleGrantsRevoke`
  - `POST /calls/open`   → `handleOpenCall`
  - `GET  /pairings/:id/watch` → `handlePairingWatch`
- 中间件：`extractBearerGrantSession(req) → grantRow`、`requirePoP(req, expectedFp?) → fp`。
- 老路径访问统一返回 `{error:"protocol_version_not_supported"}` 410。

### 5.3 `remote-invoke/service.ts`

- `submitGrantDecision`（L406-551）：approved 分支
  - 不再写 `caller_ephemeral_pub` / `client_ephemeral_pub` 到 SSE。
  - 生成 `claim_token = randomHex(32)`、`watch_token = randomHex(32)`（watch_token 在创建 pairing 时已生成，这里只是再确认有效期）；DAO `setPairingClaimTokens(...)`。
  - 推 watcher 的 payload = `{ type:"approved", claim_token, claim_expires_at, grant_summary }`。
- 新增：
  - `mintClaimToken(pairingId): {claim_token, claim_expires_at}`
  - `redeemClaim(body): {grant_session_token, expires_at, grant_summary}`
  - `mintGrantSessionToken(grantId): {token, expires_at}`
  - `resolveGrantBySessionToken(bearer): grantRow | null`
  - `revokeGrantByBearer(bearer, callerFp): void`
- `openCall`（L588-737）：签名改为 `openCall(grant: RemoteInvokeGrantRow, req: OpenCallRequestV5)`；不再读 `req.grant_id`；caller_pubkey_fp 一致性 router 已做。

### 5.4 `remote-invoke/sse.ts`

- `pairingWatchers: Map<string, Set<{res, pairingId, watchTokenHash}>>`；`registerPairingWatcher` → add；`unregisterPairingWatcher(pairingId, res?)`。
- `pushToPairingWatcher` 改成遍历 set；写失败的 entry 单独删除。
- `pushToClient` L78-87 三条 `console.log` 删除；保留 `debug` 级别的 `event` 名 + `client_instance_id`，禁止打印 payload 与 token。

### 5.5 `index.ts` 入口

- 启动日志加 `protocol_version=v5`；`/v5/remote-invoke/` 限流 key 改 `pop.callerPubkeyFp + client_instance_id`。

### 5.6 Rust caller 端 `crates/bifrost-cli/src/commands/remote.rs`

- 新增 ed25519 长期密钥管理：复用现有 `~/.bifrost/remote-device.key`（已是 Ed25519 PKCS8）作为 caller 长期身份；首次使用时在 `caller-identity.json` 写 `caller_pubkey_b64`。
- 新增 `client/pop.rs`：
  - `canonical_json` Rust 版（与 TS 一致）。
  - `sign_envelope(body_json, key) -> envelope_json`：注入 `ts/nonce/caller_pubkey/signature`。
- 替换所有 v4 调用：
  - L5238 `grants/reusable` GET → `POST /v5/grants/lookup`（PoP body），失败→`None`，成功→存 `grant_session_token`。
  - L5203 `DELETE grants/:id` → `POST /v5/grants/revoke`（Bearer+PoP）。
  - L5404 `POST calls/open` → `POST /v5/calls/open`（Bearer+PoP，body 去掉 `grant_id`，新增 `client_instance_id`）。
  - L5283 `pairings/:id/watch` → 查询参数从 `token` 改 `watch_token`；watch_token 由 `/v5/pairings/start` 响应返回。
- 测试 mock（`#[cfg(test)] mod` L9706+）：升级到 v5 路径。
- caller 本地缓存 `~/.bifrost/remote-connections.json`：新增 `grant_session_token`（加密存储，复用 `remote-connections.key`）；过期后自动 `lookup` 续。

---

## 六、测试矩阵

### 6.1 vitest 单元（`packages/bifrost-sync-server/src/__tests__/`）

| 文件 | 用例 |
|---|---|
| `pop.test.ts` 新 | 合法签名通过 / 篡改字段失败 / nonce 重放失败 / ts 超窗失败 / 非 ed25519 失败 / canonical 排序稳定 / 不含 signature 字段 |
| `grants-claim.test.ts` 新 | approved 后 claim 成功 + grant 绑定 caller_pubkey / 重复 claim 失败 / 过期 claim_token 失败 / 错配 caller_pubkey 失败 |
| `grants-lookup.test.ts` 新 | active grant → 颁发 token / 无 grant → 404 / 撤销后查不到 / 不同 caller_pubkey 隔离 |
| `grants-revoke.test.ts` 新 | Bearer + PoP 通过 → revoke / Bearer 错 → 401 / 跨 caller 不能 revoke |
| `sse-multi-watcher.test.ts` 新 | 同 pairing 多 watcher 都能收到 approved 事件 / watch_token 错误 → 401 |
| `remote-invoke-security.test.ts` 改 | v4 路径返回 410；v5 缺签名返回 401 |
| `remote-invoke-relay-v2-phase1.test.ts` 改 | open call 不再接受 grant_id 字段 |

### 6.2 e2e

- `crates/bifrost-e2e/src/tests/remote_shell_exec.rs`：升级到 v5。
- 新增 `crates/bifrost-e2e/src/tests/remote_invoke_pop.rs`：完整 pair → claim → lookup → open → events 链路；approved 事件 payload 验证不含 grant_id；revoke 后 open 失败。

### 6.3 human_tests

新建 `human_tests/remote-invoke-v5-pop.md`，索引 `human_tests/readme.md` 同步：
- TC-V5-01：pair → 桌面端审批 → CLI 自动 claim 拿 token → exec 成功。
- TC-V5-02：偷到旧 grant_id 直接 curl `/v5/calls/open` → 401。
- TC-V5-03：服务端日志 `grep -E 'token|secret|claim_token|grant_session'` → 仅出现 sha256 哈希或字面 `<redacted>`，不出现明文。
- TC-V5-04：同 pairing 两个 watcher 都收到 approved。
- TC-V5-05：revoke 后立刻 open call → 401。
- TC-V5-06：旧 v0.0.127 CLI 连新 sync server → `protocol_version_not_supported`。

### 6.4 覆盖率

- `pnpm -C packages/bifrost-sync-server coverage` 必须 ≥ 90%（pop.ts、grants-*.ts、sse 多订阅者分支全覆盖）。
- Rust 侧 `bash scripts/ci/coverage-all.sh -p bifrost-cli --json --gate` 通过。
- 收尾 `make coverage`。

---

## 七、切换策略

1. sync server 版本号 bump minor（`0.X.0` → `0.(X+1).0`）。
2. 启动时若检测旧 schema → drop & recreate 所有 `bifrost_remote_invoke_*` 表。
3. CLI 版本号同步 bump；旧客户端访问 v5 → `protocol_version_not_supported`。
4. 部署窗口：停老 server → drop 表 → 升 server → 升客户端。不允许灰度并行。

---

## 八、残余风险

| 风险 | 缓解 |
|---|---|
| caller 私钥泄露 | 文档建议用 ssh-agent / OS keychain 托管；revoke key 一键失效 |
| `grant_session_token` 泄露 | 30 min 短 TTL + 任意 revoke 即失效 |
| nonce 表膨胀 | 周期任务清理 `seen_at < now - 120s` |
| watch_token 泄露 | pairing 生命周期 ≤10 min；claim 后 watcher 自动 unregister |

---

## 九、实施清单（TodoWrite 对齐）

1. 设计稿 review 通过（本文件）。
2. Schema + DAO + types 同步。
3. pop.ts 工具 + 单测。
4. service.ts handler 改造 + 单测。
5. routes/remote-invoke.ts 路由切换 + 单测。
6. sse.ts 多订阅者 + 日志卫生。
7. Rust caller 端 commands/remote.rs + mock test。
8. e2e 新增 remote_invoke_pop。
9. human_tests 新文档。
10. pnpm lint/typecheck/test/test:e2e/coverage 全绿；`make coverage` 全绿。
11. 两轮 Review/Fix/Test。
12. commit + push + MR + 远端 CI 看护到全绿。
