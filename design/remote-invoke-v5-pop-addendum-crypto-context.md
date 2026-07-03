# Remote Invoke v5 加密上下文交换补丁（Ephemeral Pub 交换与冻结）

> 实施状态（2026-06-30 首版，2026-07-03 复核）：本设计对 v5 PoP 主稿留下的 `caller_ephemeral_pub` / `client_ephemeral_pub` 交换缺口做了补丁；相关字段、错误码 (`caller_ephemeral_pub_invalid` / `caller_ephemeral_pub_required` / `ephemeral_pub_rotation_not_allowed`) 与 vitest 用例（`grants-claim.test.ts` / `grants-lookup.test.ts`）已合入 `packages/bifrost-sync-server`，caller 侧 `caller_ephemeral_pub` 也在 claim / lookup 请求体中带上。

## 背景

原 v5 PoP 主稿（`design/remote-invoke-v5-pop.md`）§3.3 / §5.3 定义 caller open-call 命令加密依赖 X25519 ECDH：
`k = ECDH(caller_eph_priv, client_eph_pub)`。但主稿一方面禁止 `pairing.approved` SSE 广播明文 `grant_id` / `ephemeral_pub`，另一方面又没说明双方 ephemeral pub 应在哪条 authenticated 信道交换，导致实现难以对齐。

同时，reusable grant 场景下 caller 端 CLI 会重启，`caller_eph_priv` 可能丢失或轮换。这次 lookup 请求应该带上一个新的 `caller_ephemeral_pub`，但没有明确规定 “何时允许覆盖旧值、覆盖是否需要 client 二次同意”。

P0-2 硬化方案（`design/remote-invoke-v5-pop-hardening.md`）后来把 “lookup 静默轮换” 判定为漏洞并要求冻结，本补丁与之协同：**claim 时首次绑定、lookup 时严格冻结**。

## 用户目标验证清单

### 必须实现

- caller 侧 X25519 ephemeral pub 通过 `POST /v5/remote-invoke/grants/claim` 的 PoP 保护 body 上行；PoP 签名覆盖整个 body（含 ephemeral pub）。
- client 侧 ephemeral pub 通过 `claim` 与 `lookup` 的 HTTP 响应 `grant_summary.client_ephemeral_pub` 下行（TLS + PoP 保证真实性）。
- `pairing.approved` SSE 事件 **不** 携带 `grant_id`、`caller_ephemeral_pub`、`client_ephemeral_pub`、`caller_fingerprint`；只含 `{ type, claim_token, claim_expires_at, grant_summary{scope,mode,file_access} }`。
- `pairing.start` 时 client 端 ephemeral pub 直接由 client 上行（v4 已存在），server 复用即可。
- `caller_ephemeral_pub` 首次绑定后写入 `bifrost_remote_invoke_grants.caller_ephemeral_pub` 列；后续与 P0-2 一致，`lookup` 提交的 `caller_ephemeral_pub` 与库内不一致时抛 `ephemeral_pub_rotation_not_allowed`。
- claim / lookup 请求缺 `caller_ephemeral_pub` → `caller_ephemeral_pub_required`；非法 X25519 → `caller_ephemeral_pub_invalid`。
- `POST /v5/calls/open` 请求体中不再重复携带 `grant_id` 与 `caller_ephemeral_pub`（server 已存），仅通过 `Authorization: Bearer <grant_session_token>` 关联 grant。

### 必须不破坏

- v4 caller / relay 现有交互继续通过 v4 端点，本补丁不做双向兼容（v4 caller 调 v5 端点仍返回 `protocol_version_not_supported`）。
- open_call 侧派生函数（`x25519+chacha20poly1305` / HKDF）沿用 v4 现有实现，不再单独重构。
- `remote-connections.json` 中已加密保存的 `caller_eph_priv` / `client_ephemeral_pub` 结构保持兼容。

### 必须真实验证

- vitest 覆盖 claim / lookup 缺字段、非法 pub、正常 200 三条路径。
- vitest 覆盖 lookup 冻结场景，任何轮换尝试抛 `ephemeral_pub_rotation_not_allowed`。
- E2E `remote_invoke_pop.rs`：`approved` SSE payload 不含敏感字段；`claim` 响应包含 `client_ephemeral_pub`；open call 端到端加密链路通。

## 产品语义

- `caller_ephemeral_pub` 是 caller 每个 pairing session 一次性生成的 X25519 pub；私钥不落盘或仅加密落盘。
- `client_ephemeral_pub` 是 target client 在 pairing 建立时上行的 X25519 pub，server 保存并对同一 grant 生命周期内保持稳定。
- claim 与 lookup 的响应 `grant_summary.client_ephemeral_pub` 是 caller 端复用同一 grant 时唯一的 client ephemeral 来源。
- reusable grant 语义是 “同一 caller 长期使用”，因此 `caller_ephemeral_pub` 首次绑定后必须冻结；任何轮换都必须走独立的、双方 UI 都可感知的显式流程（详见 hardening 稿 P0-2）。

## 技术细节

### `POST /v5/remote-invoke/grants/claim` 请求

```jsonc
{
  "ts": 1718650000000,
  "nonce": "32-hex",
  "caller_pubkey": "<base64 SPKI DER>",
  "signature": "<base64 ed25519 sig>",
  "client_instance_id": "...",
  "pair_code": "ABC123",
  "claim_token": "<32-hex>",
  "caller_ephemeral_pub": "<base64 X25519 pub>"
}
```

server 处理链路（`packages/bifrost-sync-server/src/remote-invoke/service.ts`）：

1. `requirePoP(envelope)` 校验 PoP 签名与时间窗；
2. `getPairingByClaimTokenHash(sha256Hex(claim_token))` 定位 pairing；
3. `updateGrantCallerPubkey(grant_id, caller_pubkey, caller_pubkey_fp)`；
4. `caller_ephemeral_pub` 缺失 → 抛 `caller_ephemeral_pub_required`；解析失败或长度非 32 → `caller_ephemeral_pub_invalid`（同 hardening 稿 `service.ts:485` / `service.ts:798`）；
5. 写入 `bifrost_remote_invoke_grants.caller_ephemeral_pub`（v4 已有列，无 schema 变化）；
6. `markPairingClaimed` + `mintGrantSessionToken`。

### `POST /v5/remote-invoke/grants/claim` 响应

```jsonc
{
  "grant_session_token": "<32-hex>",
  "expires_at": "2026-06-29T01:00:00Z",
  "grant_summary": {
    "scope": "remote_query",
    "mode": "reusable",
    "file_access": "read_write",
    "client_ephemeral_pub": "<base64 X25519 pub>"
  }
}
```

响应 **不** 返回 `grant_id`、**不** 返回 `caller_ephemeral_pub`（caller 自己已有）。

### `POST /v5/remote-invoke/grants/lookup` 请求

```jsonc
{
  "ts": ..., "nonce": ..., "caller_pubkey": ..., "signature": ...,
  "client_instance_id": "...",
  "caller_ephemeral_pub": "<base64 X25519 pub>"
}
```

server：

- `getGrantByCallerFp(callerPubkeyFp, client_instance_id)`；
- grant 不存在 / 已过期 → 抛 `grant_not_found`；
- 与库内 `caller_ephemeral_pub` 不一致 → 抛 `ephemeral_pub_rotation_not_allowed`（P0-2）；
- 库内为空（legacy migration）且请求有值 → 首次绑定后允许；
- `mintGrantSessionToken` 并在响应中带回 `grant_summary.client_ephemeral_pub`。

### `POST /v5/calls/open` 请求

```jsonc
{
  "ts": ..., "nonce": ..., "caller_pubkey": ..., "signature": ...,
  "client_instance_id": "...",
  "command_summary": { ... },
  "command_kind": "...",
  "command_encrypted": {
    "alg": "x25519+chacha20poly1305",
    "ciphertext_b64": "...",
    "iv_b64": "..."
  },
  "pty_enabled": false,
  "timeout_hint_ms": 60000
}
```

- 不含 `grant_id`、不含 `caller_ephemeral_pub`（server 已保存）。
- `Authorization: Bearer <grant_session_token>` 头由 server 端反查 grant。
- server：`extractBearerGrantSession` → `requirePoP(fp == grant.caller_pubkey_fp)` → `service.openCall(grant, req)` → 内部把 `grant.grant_id + grant.caller_ephemeral_pub + command_encrypted` 推给 target。

### `pairing.approved` SSE 事件

保持主稿约束，仍然只含：

```jsonc
{
  "type": "pairing.approved",
  "claim_token": "...",
  "claim_expires_at": "...",
  "grant_summary": { "scope": "...", "mode": "...", "file_access": "..." }
}
```

不包含 `grant_id` / `caller_ephemeral_pub` / `client_ephemeral_pub` / `caller_fingerprint`。

## 错误码

| code | 含义 |
|------|------|
| `caller_ephemeral_pub_required` | claim / lookup 缺字段 |
| `caller_ephemeral_pub_invalid` | 解析失败 / 不是 X25519 / 长度错误 |
| `ephemeral_pub_rotation_not_allowed` | lookup 提交的 pub 与库内 frozen 值不一致（详见 hardening 稿 §2） |

## Schema 影响

无。`bifrost_remote_invoke_grants.caller_ephemeral_pub` 列在 v4 已存在，本补丁仅补齐 API / 语义。

## CLI

`crates/bifrost-cli/src/commands/remote.rs`：

- caller 每次 claim / lookup 前本地生成一把 X25519 ephemeral keypair（沿用 v4 helper）；
- claim / lookup body 携带 `caller_ephemeral_pub`；
- 从响应 `grant_summary.client_ephemeral_pub` 取出 client 端 pub；
- open-call 时用 `ECDH(caller_eph_priv, client_eph_pub)` + HKDF 派生 key（沿用 v4 派生函数）；
- `remote-connections.json` 加密保存：`grant_session_token`、`caller_eph_priv_b64`、`client_eph_pub_b64`、`expires_at`。
- 如需触发 caller ephemeral 轮换：CLI 必须提示用户 “请重新 `remote conn up`”，不再走 lookup 静默轮换。

## Web / Admin API

- 本补丁不引入新的 admin API；`RemoteInvokeTab` UI 不需要感知 ephemeral pub。
- `packages/bifrost-sync-server/src/routes/remote-invoke.ts` 已注册 `/v5/remote-invoke/grants/claim` 与 `/v5/remote-invoke/grants/lookup`；本补丁仅补充字段与错误码。

## Sync 边界

- ephemeral pub 属于 relay grant 生命周期，不进入 sync 层广播。
- caller 端 `caller_eph_priv` 只保存在本机加密文件，不 sync。

## Phase 拆分

- Phase 1：service 层增加 `caller_ephemeral_pub` 字段读写与错误码；claim / lookup 响应补 `client_ephemeral_pub`。
- Phase 2：CLI 端 claim / lookup 请求体填字段；`remote-connections.json` 保存 client_eph_pub。
- Phase 3：`pairing.approved` SSE payload 收敛断言；补 E2E 与 vitest。
- Phase 4：与 hardening 稿 §2 联调，冻结 lookup 轮换。

## 测试方案

### vitest

`packages/bifrost-sync-server/src/__tests__/grants-claim.test.ts`（`describe('remote invoke v5 grants claim')`）：

- `requires caller_ephemeral_pub in the PoP-protected claim body`（`grants-claim.test.ts:24`）
- `rejects caller_ephemeral_pub that is not a 32-byte base64 X25519 public key`（`grants-claim.test.ts:41`）
- `binds caller pubkey and caller ephemeral pub, then returns client_ephemeral_pub in the grant summary`（`grants-claim.test.ts:59`）

`packages/bifrost-sync-server/src/__tests__/grants-lookup.test.ts`（`describe('remote invoke v5 grants lookup')`）：

- `requires caller_ephemeral_pub`（`grants-lookup.test.ts:24`）
- `returns 404 when no active grant matches the PoP caller`（`grants-lookup.test.ts:39`）
- `mints a session token and includes client_ephemeral_pub when caller_ephemeral_pub matches the frozen one`（`grants-lookup.test.ts:50`）
- `garbage collects PoP nonces older than 60 seconds before marking the new nonce`（`grants-lookup.test.ts:71`）
- `rejects caller_ephemeral_pub rotation once frozen (P0-2)`（`grants-lookup.test.ts:98`）

### E2E

`e2e-tests/tests/test_remote_invoke_e2e.sh` 与 hardening 稿 `remote_invoke_pop.rs` E2E：

- `approved` SSE payload 断言不含 `grant_id` / `caller_ephemeral_pub` / `client_ephemeral_pub` / `caller_fingerprint`；
- `claim` 响应断言含 `client_ephemeral_pub`；
- open call 端到端加密链路通过；
- lookup 覆盖 caller 端重启后带同一 ephemeral pub 的成功路径，以及尝试轮换的失败路径。

### human_tests

- `human_tests/remote-invoke.md`：新增或扩展 “v5 PoP claim / lookup ephemeral 冻结” 用例。
- `human_tests/remote-invoke-v5-pop-hardening.md`：与 P0-2 联动的 “ephemeral 轮换必须显式流程” 用例。
- 同步刷新 `human_tests/readme.md` 用例索引。

### 校验命令

- `pnpm --filter @bifrost/sync-server test -- grants-claim grants-lookup`
- `pnpm --filter @bifrost/sync-server test:e2e`
- `bash e2e-tests/tests/test_remote_invoke_e2e.sh`
- `cargo test -p bifrost-cli --features remote`

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核 claim / lookup 请求 / 响应字段完整；PoP 覆盖整个 body；
- 复核 `pairing.approved` SSE 载荷未泄漏敏感字段；
- 复测 vitest claim / lookup 全部用例 + hardening `p0-hardening.test.ts` 中 P0-2 相关用例。

### 第 2 轮

- 复核 CLI 端 ephemeral pub 生成、保存、复用；
- 复核 open call 派生 key 与 hardening 稿保持一致；
- 复测 E2E 全链路。

## 向后兼容

依旧硬切：v4 caller 调用任何 v5 端点 → `protocol_version_not_supported`。v4 端点保持不受影响。若需要升级，caller 必须一次性升级到 v5 CLI。
