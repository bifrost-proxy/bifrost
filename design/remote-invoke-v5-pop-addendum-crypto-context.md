# 设计稿增补：v5 加密上下文交换（解决 ephemeral_pub 矛盾）

> 原设计稿 §3.3 / §5.3 留了一个未明确的口子：caller 端 open-call 命令加密依赖 X25519 ECDH（`caller_ephemeral_pub` ↔ `client_ephemeral_pub`），但 §3.3 禁止 approved SSE 广播这两个字段，又没说明应在哪条信道交换。本补丁补齐该口子。

---

## 一、原则

1. `grant_id` 仍然**不**对 caller 暴露（caller 只用 `grant_session_token`；server 端内部由 token 反查 grant_id 再下发 client）。
2. `caller_ephemeral_pub` / `client_ephemeral_pub` **不**在 SSE 明文广播。
3. 但 ECDH 需要双方 pub，因此通过下列两条 **authenticated 信道** 交换：
   - **caller → server**：`caller_ephemeral_pub` 写在 `POST /v5/grants/claim` 或 `POST /v5/grants/lookup` 的 **PoP body** 中。PoP 签名覆盖整个 body（含 ephemeral pub），server 入库前可信。
   - **server → caller**：`client_ephemeral_pub` 写在 `claim` / `lookup` 的 **响应 body** 中（HTTP 响应已经由 PoP 流程 + TLS 提供端到端真实性）。
4. `caller_ephemeral_pub` 的处理与现有 v4 相同（每个 grant 一次性，绑定后写入 `bifrost_remote_invoke_grants.caller_ephemeral_pub`）。
5. **`/v5/grants/lookup`**（复用 reusable grant）：若 caller 的 ephemeral 私钥已丢失（如 CLI 重启），允许 caller 提交一把**新**的 `caller_ephemeral_pub` —— server 校验 `caller_pubkey == grant.caller_pubkey`（PoP fp 一致）后覆盖。reusable grant 的语义是"同一 caller 重复使用"，ephemeral 是 per-session 的，覆盖是安全的。
6. `client_ephemeral_pub` 在 pairing 创建时由 client 通过现有 `pairing.start` 上行（v4 已存在），server 直接复用。

---

## 二、协议字段补丁

### 2.1 `POST /v5/grants/claim` request body

```jsonc
{
  "ts": 1718650000000,
  "nonce": "32-hex",
  "caller_pubkey": "<base64 SPKI DER>",     // 长期 Ed25519 身份
  "signature": "<base64 ed25519 sig>",
  "client_instance_id": "...",
  "pair_code": "ABC123",
  "claim_token": "<32-hex>",
  "caller_ephemeral_pub": "<base64 X25519 pub>"  // ← 新增：本会话 ECDH pub
}
```

server 校验 PoP → 校验 claim_token_hash → 校验 pair_code → `updateGrantCallerPubkey(grant_id, caller_pubkey, caller_pubkey_fp)` → 同时写入 `caller_ephemeral_pub`（沿用 v4 列）→ `markPairingClaimed` → `mintGrantSessionToken`。

### 2.2 `POST /v5/grants/claim` response

```jsonc
{
  "grant_session_token": "<32-hex>",
  "expires_at": "2026-06-29T01:00:00Z",
  "grant_summary": {
    "scope": "remote_query",
    "mode": "reusable",
    "file_access": "read_write",
    "client_ephemeral_pub": "<base64 X25519 pub>"   // ← 新增：用于 caller ECDH
  }
}
```

注意：仍然**不**返回 `grant_id`、不返回 `caller_ephemeral_pub`（caller 自己有）。

### 2.3 `POST /v5/grants/lookup` request

```jsonc
{
  "ts": ..., "nonce": ..., "caller_pubkey": ..., "signature": ...,
  "client_instance_id": "...",
  "caller_ephemeral_pub": "<base64 X25519 pub>"   // ← 新增：每次 lookup 允许更新
}
```

server：getGrantByCallerFp → 校验 active → 若 `caller_ephemeral_pub` 与库内不同则覆盖 → mint session token → 返回 `grant_summary { ..., client_ephemeral_pub }`。

### 2.4 `POST /v5/calls/open` request

```jsonc
{
  "ts": ..., "nonce": ..., "caller_pubkey": ..., "signature": ...,
  "client_instance_id": "...",
  "command_summary": { ... },
  "command_kind": "...",
  "command_encrypted": {                  // caller 用 ECDH(caller_eph_priv, client_eph_pub) 派生 key 加密
    "alg": "x25519+chacha20poly1305",
    "ciphertext_b64": "...",
    "iv_b64": "..."
  },
  "pty_enabled": false,
  "timeout_hint_ms": 60000
}
```

不含 `grant_id`、不含 `caller_ephemeral_pub`（server 已存）。`Authorization: Bearer <grant_session_token>` 头部传入。

server：extractBearerGrantSession → requirePoP（fp == grant.caller_pubkey_fp） → `service.openCall(grant, req)` → 内部把 grant.grant_id + grant.caller_ephemeral_pub + command_encrypted 推给 client（不变）。

### 2.5 `pairing.approved` SSE event

保持原 §3.3：**不**含 grant_id、不含 ephemeral pub、不含 caller fp；只含 `{type, claim_token, claim_expires_at, grant_summary{scope,mode,file_access}}`。caller 调 `/v5/grants/claim` 后才拿到 `client_ephemeral_pub`。

---

## 三、错误码补丁

| code | 含义 |
|---|---|
| `caller_ephemeral_pub_invalid` | 解析失败 / 不是 X25519 / 长度错误 |
| `caller_ephemeral_pub_required` | claim/lookup 缺字段 |

---

## 四、Schema 影响

无新增字段（`bifrost_remote_invoke_grants.caller_ephemeral_pub` v4 已存在）。

---

## 五、CLI 改造影响（追加到原 §5.6）

- caller 每次 claim/lookup 前**本地生成**一把 X25519 ephemeral keypair（已有），私钥不落盘或加密落盘。
- claim/lookup body 携带 `caller_ephemeral_pub`。
- 从响应 `grant_summary.client_ephemeral_pub` 取出 client 端 pub。
- open-call 时用 `ECDH(caller_eph_priv, client_eph_pub)` + HKDF 派生 key（沿用 v4 现有派生函数，复用 `caller_eph_priv` 不变）。
- `remote-connections.json` 加密存：`grant_session_token`、`caller_eph_priv_b64`、`client_eph_pub_b64`、`expires_at`。

---

## 六、测试补丁（追加到 §6.1）

- `pop.test.ts`：略，不变。
- `grants-claim.test.ts`：
  - 缺 `caller_ephemeral_pub` → 400 `caller_ephemeral_pub_required`
  - 非 X25519 pub → 400 `caller_ephemeral_pub_invalid`
  - 成功 → 返回包含 `client_ephemeral_pub`
- `grants-lookup.test.ts`：
  - 第二次 lookup 传不同 `caller_ephemeral_pub` → server 覆盖，新 session token 通过
  - 响应含 `client_ephemeral_pub`
- E2E `remote_invoke_pop.rs`：
  - approved SSE payload 断言不含 `grant_id` / `caller_ephemeral_pub` / `client_ephemeral_pub`
  - claim 响应断言含 `client_ephemeral_pub`
  - open call 端到端加密链路通

---

## 七、向后兼容

依旧硬切，v4 client 调用任何 v5 端点 → `protocol_version_not_supported`。
