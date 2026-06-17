# Remote Invoke 安全重构方案

> 状态：**已上线，部分演进于实现中** | 创建时间：2026-04-20 | 上次刷新：2026-06-16
>
> **2026-06-16 实现核对要点（与原文差异概览）**：
> - Relay 实现已整体迁移到 `packages/bifrost-sync-server`，原 `bifrost-server-v4/app/**` 路径已不存在；本文中所有 `bifrost-server-v4/app/...` 路径仅保留作为历史背景，等价代码请参见 `packages/bifrost-sync-server/src/routes/remote-invoke.ts` 与 `packages/bifrost-sync-server/src/remote-invoke/service.ts`。
> - `GET /v4/remote-invoke/clients`、Caller 侧 `GET /v4/remote-invoke/grants`、Caller 侧 `PATCH /v4/remote-invoke/grants/:id` 均已从 Relay 路由表移除（在当前实现中只保留了 `client/grants/:id` PATCH，由已 token 鉴权的 Client 侧使用）。
> - 配对/调用通道在原本「fingerprint + grant_id 双因子」的基础上额外引入了 X25519 ephemeral key 派生的共享密钥与命令端到端加密（`caller_ephemeral_pub` / `client_ephemeral_pub` / `command_encrypted`），并新增了 SSH key 复用配对路径（`/ssh/challenge` + `/ssh/connect`）。下文相关章节就近补注。
> - `caller_fingerprint` 已不再用 `simple_hash(username+hostname)`，而是 16 字节随机数 `caller-<hex32>`，独立持久化在 `caller-identity.json`（详见第七节）。
> - `LocalConnection` 结构、`handle_connect` 行为相比原文有扩展（增加 ephemeral key / transport context / auth_method / ssh_key_* / device_code 等字段）。

## 一、问题背景

### 1.1 当前安全漏洞

**核心问题：`GET /v4/remote-invoke/clients` 完全无鉴权，任何人都可以枚举 Relay 上所有在线 Bifrost 实例。**

> 历史背景：原文成稿时 Caller 路由位于 `bifrost-server-v4/app/routes/remoteInvoke.ts`；目前等价实现在 `packages/bifrost-sync-server/src/routes/remote-invoke.ts`，下面表格描述的是原 v4 时期的问题面，已在新实现中按方案修复。

当前 Caller 路由组（`registerCallerRoutes`，位于 `bifrost-server-v4/app/routes/remoteInvoke.ts:223`）包含以下 **全部无鉴权** 的端点：

| 端点 | 方法 | 风险 |
|---|---|---|
| `/v4/remote-invoke/clients` | GET | 枚举所有在线客户端（设备名、平台、client_id） |
| `/v4/remote-invoke/grants` | GET | 查询任意客户端的授权列表 |
| `/v4/remote-invoke/grants/reusable` | GET | 探测任意客户端是否存在有效授权 |
| `/v4/remote-invoke/grants/:id` | PATCH | 修改任意授权 |
| `/v4/remote-invoke/grants/:id` | DELETE | 撤销任意授权 |
| `/v4/remote-invoke/calls/open` | POST | 用 grant_id 发起远程调用 |
| `/v4/remote-invoke/pairings/start` | POST | 仅需 pair_code（本身相对安全） |

### 1.2 当前执行流程（不安全）

```
bifrost remote status [--client-id <前缀>]
  │
  ├─ 1. resolve_client_id()
  │     → GET /v4/remote-invoke/clients        ← 无鉴权，返回全部在线客户端
  │     → 前缀匹配 / 交互选择 / 自动选唯一
  │
  ├─ 2. find_reusable_grant()
  │     → GET /grants/reusable?cid=X&fp=Y      ← 无鉴权
  │     → 找到 grant → 继续执行
  │     → 未找到 → 报错 "请先 connect"
  │
  └─ 3. open_call()
        → POST /calls/open {grant_id, ...}     ← 只验 grant_id 存在性，不验调用者身份
```

**问题：即使 Step 2 要求先有 grant，Step 1 已经泄露了所有客户端信息。`disconnect --all` 也通过 `resolve_client_id` 和 `list_grants` 无鉴权列出授权。**

### 1.3 设计文档 vs 实现的差距

设计文档（`design/remote-command-bridge.md`）已经描述了正确的安全模型：
- 6.1 节要求 `GET /clients` 需要 `x-bifrost-token`
- 风险 9 要求客户端列表按用户隔离

但实现完全没有遵循——Caller 路由组无任何鉴权中间件。

---

## 二、改造目标

### 2.1 核心设计哲学

**Relay 是透明中继服务，不管理 Caller 身份（Caller 无需任何鉴权 token）。**

安全模型的两根支柱：

1. **客户端可见性管控** — Relay 绝不主动暴露注册的客户端信息。pair_code 是唯一的客户端发现机制。
2. **Client 主动授权** — 只有被调用客户端审批通过后，才会向 Caller 释放 client_id、设备信息以及 grant。

这意味着：
- Caller 不需要注册、不需要登录、不需要 token
- Client 注册仍需 `x-bifrost-token`（保持现有机制不变）
- `grant_id` 本身就是操作凭证（UUID 不可猜测 + 绑定 `caller_fingerprint`）。注：当前实现中 `caller_fingerprint` 已是 16 字节随机数（`caller-<hex32>`），并落盘 `caller-identity.json`；不再是 `username+hostname` 的派生值（见第七节决策 #5 更新）。

### 2.2 目标流程

```
┌──────────────────────────────────────────────────────────────────────────┐
│ 第一步：带外分享 pair_code                                                │
│                                                                          │
│  Client 开启 discovery → 屏幕/WebUI 显示 pair_code（6 位, 2 分钟 TTL）     │
│  Client 主人通过微信/口头/截屏等方式，将 pair_code 告知 Caller 用户          │
└──────────────────────────────────────────────────────────────────────────┘
                                  │
                                  ▼
┌──────────────────────────────────────────────────────────────────────────┐
│ 第二步：Caller 使用 pair_code 配对                                        │
│                                                                          │
│  bifrost remote connect <pair_code>                                      │
│    → POST /pairings/start {pair_code, caller_info}                       │
│    → Relay 通过 pair_code 解析出 client_instance_id（Caller 不可见）       │
│    → Client 弹窗审批 → 批准/拒绝                                          │
│    → 批准后 Relay 返回 {grant_id, client_instance_id,                     │
│                          device_name, platform}                          │
│    → CLI 将连接信息保存到本地 {BIFROST_DATA_DIR}/remote-connections.json   │
└──────────────────────────────────────────────────────────────────────────┘
                                  │
                                  ▼
┌──────────────────────────────────────────────────────────────────────────┐
│ 第三步：执行命令（基于本地连接状态 + grant 凭证）                             │
│                                                                          │
│  bifrost remote status [--client-id <前缀>]                               │
│    → 从本地 remote-connections.json 解析 client_id 和 grant_id            │
│    → 查询 grant 有效性:                                                   │
│      GET /grants/reusable?cid=X&fp=Y&grant_id=G                         │
│    → 执行命令:                                                            │
│      POST /calls/open {grant_id, client_instance_id, caller_fingerprint} │
│    → Relay 校验 grant 存在 + caller_fingerprint 匹配                      │
│    → SSE 返回结果                                                         │
└──────────────────────────────────────────────────────────────────────────┘
```

---

## 三、安全模型详解

### 3.1 认证分层

```
┌─────────────────────────────────────────────────────┐
│              Relay Server（透明中继）                  │
│                                                     │
│  Client 路由   →  x-bifrost-token 鉴权（已有，不变）   │
│                   用于：注册、SSE 连接、配对审批        │
│                                                     │
│  Caller 路由   →  无身份鉴权（透明）                   │
│                   可见性管控：                         │
│                     · GET /clients → 删除            │
│                     · 配对 → pair_code 门控           │
│                   操作鉴权：                           │
│                     · grant_id + caller_fingerprint  │
│                     · Relay 校验两者绑定关系           │
└─────────────────────────────────────────────────────┘
```

### 3.2 安全闸门链

```
pair_code (6位, 2min TTL, 一次性)
    │
    ▼ Caller 提交 pair_code
    │
Client 人工审批（弹窗）
    │
    ▼ 批准
    │
grant_id (UUID, 绑定 caller_fingerprint, 有时效)
    │
    ▼ Caller 使用 grant_id + fingerprint 操作
    │
命令白名单（status, traffic.list, traffic.get, search.get, traffic.search）
```

每一层都是独立的安全屏障：
- **pair_code** — 物理/社交通道传递，Relay 不会泄露
- **人工审批** — Client 主人看到 Caller 设备名、IP、平台后决定
- **grant 绑定** — `grant_id` 不可猜测 + 绑定 `caller_fingerprint`，非授权 Caller 无法使用别人的 grant
- **命令白名单** — 即使 grant 有效，也只能执行允许的命令

### 3.3 grant 作为操作凭证的安全性分析

无 `caller_token` 时，操作鉴权完全靠 `grant_id` + `caller_fingerprint`：

| 攻击场景 | 防御 |
|---|---|
| 猜测 grant_id | UUID v4，128 位随机，不可暴力枚举 |
| 窃取 grant_id | 需同时知道 caller_fingerprint 才能使用（实现中 fingerprint 是本机随机生成的 128 位标识，等同于「caller 本地秘密」） |
| 伪造 caller_fingerprint | ~~fingerprint 基于 username+hostname 生成~~ → 实现已升级为本机 16 字节随机数（`caller-identity.json`），攻击者无法通过 username/hostname 推算 |
| 遍历客户端 | `GET /clients` 已删除，无枚举入口 |
| 重放已过期 grant | Relay 校验 grant 时效（Once/30m/1h/1d/Permanent） |

**结论：** 在 Relay 作为透明中继的场景下，`grant_id` + `caller_fingerprint` 双因子已提供足够安全性，无需引入额外的 `caller_token`。

---

## 四、具体改造点

### 4.1 Relay Server（原 `bifrost-server-v4`，现 `packages/bifrost-sync-server`）

#### 4.1.1 删除 `GET /clients` 端点

**当前（`remoteInvoke.ts:334`）：** 任何人可调用，返回所有在线客户端信息。

**改造：** 完全删除此路由，不保留任何访问入口。

_实现核对（2026-06-16）_：`packages/bifrost-sync-server/src/routes/remote-invoke.ts` 的 `handleRemoteInvoke` 路由表中已不再存在 `/v4/remote-invoke/clients` 入口（grep `remote-invoke/clients` 无匹配）。

#### 4.1.2 Caller 路由添加 grant 归属校验

Caller 路由不需要身份 token，但需要在涉及 grant 操作的端点上验证 `caller_fingerprint` 与 grant 的绑定关系。

| 端点 | 当前鉴权 | 改造后鉴权 |
|---|---|---|
| `POST /pairings/start` | pair_code | **不变**。pair_code 本身就是发现门控 |
| `GET /pairings/:id/watch` | 无 | **不变**。pairing_id 是一次性 UUID |
| `GET /grants/reusable` | 无 | **校验 `caller_fingerprint` 参数与 grant 绑定匹配** |
| `POST /calls/open` | 无 | **校验 `caller_fingerprint` 参数与 grant 绑定匹配** |
| `DELETE /grants/:id` | 无 | **校验 `caller_fingerprint` 参数与 grant 绑定匹配** |
| `GET /calls/:id/events` | 无 | **校验 call 归属**（call 创建时已绑定 caller） |
| **`GET /clients`** | **无** | **删除** |
| **`GET /grants`** | **无** | **删除**（Caller 无需列出所有 grant，只需 `/grants/reusable` 查具体授权） |
| **`PATCH /grants/:id`** | **无** | **删除**（grant 属性由 Client 审批时决定，Caller 不应修改） |

_实现核对（2026-06-16）_：`packages/bifrost-sync-server/src/routes/remote-invoke.ts` 中：
- Caller 路径 `GET /clients`、`GET /grants`、`PATCH /grants/:id` 均无对应路由表项；只剩 `client/grants/:id` 的 `PATCH`（受 Client 的 `requireClientAuth` 鉴权，给 Client 自己更新 `grant_scope` / `file_access` 用，不属于 Caller 暴露面）。
- `GET /grants/reusable`、`DELETE /grants/:id`、`POST /calls/open`、`POST /pairings/start`、`GET /pairings/:id/watch`、`GET /calls/:id/events`、`POST /calls/:id/cancel`、`POST /calls/:id/input` 仍在 Caller 路径中暴露，且按方案验证 `caller_fingerprint` / `relay_token` / pairing_id 等凭证。

**校验逻辑伪代码：**
```typescript
function validateGrantOwnership(grant: GrantRecord, callerFingerprint: string): boolean {
  return grant.caller_fingerprint === callerFingerprint;
}
```

#### 4.1.2.5 SSH key 复用配对路径（实现已上线，原文未覆盖）

实际实现中除了 pair_code 一次性配对外，还提供了基于 SSH 长期 key 的复用配对路径，用于「同一台 Caller 反复操作同一台 Client」的场景：

- `POST /v4/remote-invoke/ssh/challenge` — 由 Caller 用 `device_code`（SSH key 上携带的设备标识）领取签名挑战。
- `POST /v4/remote-invoke/ssh/connect` — Caller 用 SSH key 私钥对挑战做签名 + 携带 `caller_ephemeral_pub`，由对应 Client 弹窗审批，审批通过后下发 `grant_id` 与 `client_ephemeral_pub`。
- `POST /v4/remote-invoke/ssh/connect-result` — Client 侧上报审批结果。

Caller 侧 CLI 入口为 `bifrost remote connect --ssh-key ...`，连接成功后 `LocalConnection.auth_method` 会写成 `"ssh"` 并附带 `ssh_key_fingerprint` / `ssh_key_source` / `device_code` 字段（与 pair_code 路径共用同一份 `remote-connections.json`）。本节安全模型同样适用：grant 仍按 `caller_fingerprint` + `client_instance_id` 校验，命令仍走 `command_encrypted`。

#### 4.1.3 `/pairings/start` 改造

_实现核对（2026-06-16）_：当前 `handleStartPairing` 要求 `pair_code + caller_info + caller_ephemeral_pub` 三段；`client_instance_id` 与 `command` / `command_summary` 已按方案从入参中移除。**与原文差异**：实际入参额外要求 `caller_ephemeral_pub`（caller 端 X25519 临时公钥），用于和 `client_ephemeral_pub` 派生共享密钥，是 E2E 命令加密的前提；下文方案 JSON 仅展示了 fingerprint，请实现时以代码为准。

**当前请求体：**
```json
{
  "client_instance_id": "uuid-xxx",   // Caller 需提前知道 client_id ← 安全问题
  "pair_code": "A3K9M2",
  "caller_info": { "fingerprint": "...", "display_name": "..." },
  "command_summary": { ... },
  "command": { ... }
}
```

**改造后请求体：**
```json
{
  "pair_code": "A3K9M2",
  "caller_info": { "fingerprint": "...", "display_name": "..." }
}
```

变更说明：
- 移除 `client_instance_id` — 由 Relay 从 pair_code 自动解析
- 移除 `command_summary` / `command` — connect 阶段只做配对授权，不携带具体命令

**Relay 处理逻辑变更：**
```
1. 收到 pair_code
2. 在 Redis 中查找 ri:pair_code:{code} → 解析出 client_instance_id
3. 找不到 → 返回 404 "invalid or expired pair code"
4. 找到 → 创建 pairing 请求，通过 SSE 推送到 Client
5. Client 弹窗审批
6. 审批通过 → 创建 grant，绑定 caller_fingerprint
```

**改造后 SSE decision 事件（配对审批通过后）：**
```json
{
  "status": "approved",
  "grant_id": "grant-xxx",
  "client_instance_id": "uuid-full-id",
  "device_name": "Eden-MacBook",
  "platform": "macos",
  "grant_mode": "permanent"
}
```

_实现核对（2026-06-16）_：`service.submitGrantDecision` 返回的字段是 `{grant_id, status, client_instance_id, device_name, platform, grant_mode, grant_scope, file_access, client_ephemeral_pub}`——比原文多出 `grant_scope`、`file_access`、`client_ephemeral_pub`（Caller 侧依赖 `client_ephemeral_pub` 完成 X25519 共享密钥派生，缺它会报 `pairing succeeded but relay did not return client_ephemeral_pub required for encrypted remote commands`）。

Caller 从此事件中获得：
- `client_instance_id` — 之后用于本地记录和命令执行
- `grant_id` — 之后用于操作凭证
- `device_name` / `platform` — 用于本地展示
- `grant_mode` — 用于本地记录授权类型

#### 4.1.4 `/calls/open` 加强校验

**当前：** 只验 `grant_id` 存在。

**改造后请求体：**
```json
{
  "grant_id": "grant-xxx",
  "client_instance_id": "uuid-xxx",
  "caller_fingerprint": "fp-xxx",
  "command": { "command": "status", "args_json": null },
  "command_summary": { "command_preview": "status" }
}
```

_实现核对（2026-06-16）_：`handleOpenCall` 实际入参为 `{grant_id, client_instance_id, caller_fingerprint, command_kind, command_summary, command_encrypted, [caller_pubkey, pty_enabled, timeout_hint_ms]}`，**明文 `command` 字段已被 `command_encrypted` 取代**（用 `pairing/decision` 阶段派生出的共享密钥对命令做 AEAD 加密；relay 仅做转发，不再能看到命令明文）。fingerprint / client_instance_id 校验逻辑保留。

**Relay 校验逻辑：**
```
1. 查找 grant → 不存在返回 404
2. 校验 grant.caller_fingerprint === 请求中的 caller_fingerprint → 不匹配返回 403
3. 校验 grant.client_instance_id === 请求中的 client_instance_id → 不匹配返回 403
4. 校验 grant 未过期 → 过期返回 403
5. 校验通过 → 创建 call，转发到 Client
```

_实现核对（2026-06-16）_：`handleOpenCall` 错误分支与原文一致：`caller_fingerprint_mismatch` / `client_instance_id_mismatch` / `grant_expired` / `grant_consumed` / `grant_scope_mismatch` 均返回 403，`grant_not_found` 返回 404。

#### 4.1.5 `/grants/reusable` 加强校验

**改造后请求参数：**
```
GET /grants/reusable?client_instance_id=X&caller_fingerprint=Y
```

**Relay 校验逻辑：**
```
1. 查找匹配 {client_instance_id, caller_fingerprint} 的有效 grant
2. 只返回 caller_fingerprint 完全匹配的 grant（已有逻辑，确认即可）
3. 无匹配 → 返回 null
```

_实现核对（2026-06-16）_：`handleFindReusableGrant` 校验 `client_instance_id` 与 `caller_fingerprint` 均非空，缺一返回 400；service 层按 `(client_instance_id, caller_fingerprint)` 精确查询有效 grant，与原文方案一致。

#### 4.1.6 `DELETE /grants/:id` 加强校验

**改造后请求参数：**
```
DELETE /grants/:grantId?caller_fingerprint=Y
```

**Relay 校验逻辑：**
```
1. 查找 grant → 不存在返回 404
2. 校验 grant.caller_fingerprint === 请求参数中的 caller_fingerprint → 不匹配返回 403
3. 校验通过 → 删除 grant，通知 Client
```

### 4.2 CLI Caller 侧（`crates/bifrost-cli/src/commands/remote.rs`）

#### 4.2.1 新增本地连接文件

**文件路径：** `{BIFROST_DATA_DIR}/remote-connections.json`（默认即 `~/.bifrost/remote-connections.json`）

**结构：**
```json
{
  "version": 1,
  "connections": [
    {
      "client_instance_id": "uuid-xxx-full",
      "device_name": "Eden-MacBook",
      "platform": "macos",
      "relay_url": "https://bifrost.bytedance.net",
      "grant_id": "grant-xxx",
      "grant_mode": "permanent",
      "caller_fingerprint": "fp-xxx",
      "connected_at": 1713600000000
    }
  ]
}
```

注意：不再有 `caller_token` 字段。`grant_id` + `caller_fingerprint` 就是操作凭证。

_实现核对（2026-06-16）_：实际 `LocalConnection` 已扩展为以下字段（`crates/bifrost-cli/src/commands/remote.rs`，常量 `CONNECTIONS_FILE = "remote-connections.json"`）：基础字段同方案（`client_instance_id` / `device_name` / `platform` / `relay_url` / `grant_id` / `grant_mode` / `caller_fingerprint` / `connected_at`），并新增 `auth_method`（`"pair_code"` 或 `"ssh"`）、`ssh_key_fingerprint` / `ssh_key_source` / `device_code`（SSH 路径专用）、`transport_context_version`、`caller_ephemeral_pub` / `client_ephemeral_pub` / `shared_secret_encrypted`（命令端到端加密所需，全部 `skip_serializing_if = Option::is_none`）。`caller_token` 字段确认不存在。

_本地连接文件相关补充_：caller 还会维护两份独立文件：
- `{BIFROST_DATA_DIR}/caller-identity.json` — 持久化 `caller_fingerprint`（详见第七节差异说明）。
- `{BIFROST_DATA_DIR}/remote-connections.key` 等密钥文件（由 `CONNECTIONS_KEY_FILE` 常量驱动），用于 `shared_secret_encrypted` 的本地加解密。

#### 4.2.2 删除 `resolve_client_id()`

**当前代码（`remote.rs:364-480`）：** 调用 `GET /clients` 列出全部在线客户端，然后前缀匹配。

**替换为 `resolve_local_connection()`：**
```rust
fn resolve_local_connection(
    connections: &[LocalConnection],
    explicit_id: Option<&str>,
) -> bifrost_core::Result<LocalConnection> {
    // 从本地连接文件中按前缀匹配 client_instance_id
    // 0 个匹配 → 报错 "no saved connection, please run `bifrost remote connect <pair-code>` first"
    // 1 个匹配 → 直接返回
    // 多个匹配 → 交互选择（展示 device_name + short_id）
}
```

#### 4.2.3 改造 `handle_connect`

**当前（`remote.rs:124-204`）：** 需要 `client_instance_id` 参数。

**改造后：**
```rust
async fn handle_connect(
    caller: &CallerRelayClient,
    pair_code: &str,
    caller_info: &CallerInfo,
    relay_url: &str,
) -> bifrost_core::Result<()> {
    // 1. POST /pairings/start {pair_code, caller_info}
    //    → 不再传 client_instance_id
    //    → Relay 从 pair_code 解析
    // 2. watch_pairing SSE → 等待审批结果
    // 3. 审批通过 → 获取 {grant_id, client_instance_id, device_name, platform, grant_mode}
    // 4. 保存到 {BIFROST_DATA_DIR}/remote-connections.json
    //    如果同一个 {client_instance_id, relay_url} 已存在，更新（覆盖旧 grant）
    // 5. 打印成功信息
}
```

#### 4.2.4 改造主流程 `async_handle_remote_command`

**当前（`remote.rs:36-121`）：**
```
1. if Connect → handle_connect (需 client_id)
2. if Disconnect → handle_disconnect
3. 其他命令 → resolve_client_id() → find_grant → open_call
```

**改造后：**
```
1. if Connect → handle_connect (只需 pair_code)
2. 加载本地连接文件 remote-connections.json
3. if Disconnect → handle_disconnect (从本地文件解析)
4. 其他命令 →
   a. resolve_local_connection() → 获取 {client_id, grant_id, caller_fingerprint}
   b. find_reusable_grant(client_id, caller_fingerprint) → 验证 grant 仍有效
      - 有效 → 继续
      - 无效 → 报错 "authorization expired, please run `bifrost remote connect <pair-code>` again"
   c. open_call(grant_id, client_id, caller_fingerprint, command)
   d. subscribe_call_events → 获取结果
```

#### 4.2.5 `CallerRelayClient` 简化

**当前（`remote.rs:612-638`）：** 有 `token` 字段和 `auth_headers()` 方法发送 `x-bifrost-token`。

**改造后：**
```rust
struct CallerRelayClient {
    http: reqwest::Client,
    base_url: String,
    // 移除 token 字段 — Caller 不需要身份 token
}

impl CallerRelayClient {
    fn new(base_url: &str) -> Self { ... }
    // 移除 auth_headers() — 不再发送任何 auth header
    // 所有操作通过请求参数携带 grant_id + caller_fingerprint
}
```

_实现核对（2026-06-16）_：`CallerRelayClient` 当前定义就是 `{ http: reqwest::Client, base_url: String }`，无 `token` / `auth_headers()`，符合方案。需要注意：`POST /calls/{id}/cancel`、`POST /calls/{id}/input`、`GET /calls/{id}/events` 这一组接口仍校验 `Authorization: Bearer <relay_token>`，但这里的 `relay_token` 是 `/calls/open` 调用成功后由 relay 单独下发的「call-scoped 一次性令牌」（短时有效、仅作用于该 call），不是 caller 身份 token，与 `x-bifrost-token` 是两件事。

#### 4.2.6 `handle_disconnect` 改造

**当前（`remote.rs:206-260`）：** 调用 `resolve_client_id()` + `list_grants()` 无鉴权。

**改造后：**
```
1. 从本地连接文件解析目标 connection
2. DELETE /grants/:grant_id?caller_fingerprint=Y  → Relay 校验归属后删除
3. 成功后从本地连接文件中移除对应记录
4. --all 模式：遍历本地所有连接，逐个 disconnect 并清理
5. --grant-id 模式：直接删除指定 grant（仍需 caller_fingerprint 校验）
```

### 4.3 Client Worker 侧（`crates/bifrost-admin/src/remote_invoke/worker.rs`）

**无需改动。** 当前 Client 的 discovery 模式已经正确：
- 生成 pair_code → 发布到 Relay → 等待配对请求 → 弹窗审批
- pair_code 是唯一的发现路径（已满足）

_实现核对（2026-06-16）_：`worker.rs` 仍存在并按本方案运转。此外 `crates/bifrost-admin/src/remote_invoke/` 已扩展出 `executor.rs`（命令分发）、`file_ops.rs` / `file_access_roots.rs` / `file_policy_store.rs`（远程文件操作 + FileAccessPolicy）、`grant_crypto_store.rs` / `grant_info_store.rs` / `grant_policy_store.rs`（grant 与共享密钥本地持久化）、`session_ring.rs` / `stream_emit.rs`（长 session 流式回放与 offset 续传）、`ssh_keys.rs` / `identity.rs` 等模块；这些在原文中均未提及，但属于本方案落地后的附带能力，**不破坏本节安全模型**。

### 4.4 本地 Sync Server（`packages/bifrost-sync-server`）

同步 4.1 的所有改造到本地版本。

_实现核对（2026-06-16）_：实际架构上 `packages/bifrost-sync-server` **就是 4.1 节所说的 Relay Server 实现本身**，并非「需要单独同步的本地副本」——原文计划中的两套代码现已合并为一份。下文第八节中 `bifrost-server-v4/app/**` 路径请按 `packages/bifrost-sync-server/src/**` 等价解读（具体映射见第七节后的「实现路径映射」补注）。

---

## 五、安全对比

| 维度 | 当前 | 改造后 |
|---|---|---|
| 客户端枚举 | ❌ 任何人可列出所有在线客户端 | ✅ `GET /clients` 完全删除 |
| 发现机制 | ❌ 枚举 + 前缀匹配选择 | ✅ 仅通过 pair_code 发现特定客户端 |
| 信息泄露面 | ❌ client_id, device_name, platform 全量公开 | ✅ 仅配对审批成功后释放 client 信息 |
| Grant 操作 | ❌ 任何人可查/改/删任意 grant | ✅ `grant_id` + `caller_fingerprint` 双因子校验 |
| 命令执行 | ❌ 只验 grant_id 存在性 | ✅ 验证 grant + caller_fingerprint 绑定关系 |
| 前缀匹配 | ❌ 在全量客户端列表上匹配 | ✅ 在本地已连接列表上匹配 |
| Caller 鉴权 | ❌ 无意义的 x-bifrost-token | ✅ 不需要 token（Relay 透明中继），安全性由可见性管控 + grant 绑定保障 |

---

## 六、实施计划

### Phase 1 — Relay Server 改造（原 `bifrost-server-v4`，现 `packages/bifrost-sync-server`）

1. 完全删除 `GET /v4/remote-invoke/clients` 端点
2. 删除 `GET /v4/remote-invoke/grants`（Caller 无需列表接口）
3. 删除 `PATCH /v4/remote-invoke/grants/:id`（Caller 无需修改 grant）
4. 修改 `/pairings/start`：移除 `client_instance_id` 参数，由 Relay 从 pair_code 解析
5. 修改 `/pairings/start`：移除 `command` / `command_summary` 参数（connect 只做配对）
6. 修改配对审批成功的 SSE 事件：返回 `{grant_id, client_instance_id, device_name, platform, grant_mode}`
7. 修改 `/calls/open`：要求传入 `caller_fingerprint`，校验与 grant 绑定关系
8. 修改 `/grants/reusable`：确认已按 `caller_fingerprint` 过滤
9. 修改 `DELETE /grants/:id`：要求传入 `caller_fingerprint`，校验归属

### Phase 2 — CLI Caller 改造（`remote.rs`）

1. 新增本地连接文件（`{BIFROST_DATA_DIR}/remote-connections.json`）读写逻辑
2. 删除 `resolve_client_id()`，替换为 `resolve_local_connection()`
3. 修改 `handle_connect`：不再需要 `client_instance_id`，成功后保存连接状态到本地
4. 简化 `CallerRelayClient`：移除 `token` 字段和 `auth_headers()`
5. 所有 Relay 请求改为通过参数传递 `caller_fingerprint`（而非 header token）
6. 修改 `handle_disconnect`：从本地连接文件解析 + 清理本地记录
7. 修改 `async_handle_remote_command` 主流程

### Phase 3 — 本地 Sync Server 同步（`bifrost-sync-server`）

- 同步 Phase 1 的改造到本地版本

_实现核对（2026-06-16）_：Phase 1 与 Phase 3 已合并——`packages/bifrost-sync-server` 即是 Relay Server 唯一实现，无需额外同步动作。

---

## 七、已确认的设计决策

| # | 决策项 | 结论 |
|---|---|---|
| 1 | 本地连接文件位置 | **`{BIFROST_DATA_DIR}/remote-connections.json`**，默认即 `~/.bifrost/remote-connections.json`。完全由启动 bifrost 命令时的数据目录决定。 |
| 2 | `GET /clients` 处理方式 | **完全删除**，不保留任何形式的访问入口。 |
| 3 | `/pairings/start` 是否接受 `client_instance_id` | **完全移除**，只通过 pair_code 解析。Caller 不应在配对前知道 client_id。 |
| 4 | 重复配对（同 Caller 对同 Client 再次 connect） | 新 grant 与旧 grant 并存，本地连接文件更新为最新 grant，旧 grant 在服务端自然过期。 |
| 5 | `caller_fingerprint` 生成算法 | ~~暂保持现有 `simple_hash`~~ → **已在实现中升级**：现为 16 字节加密随机数，格式 `caller-<hex32>`（`generate_random_caller_fingerprint` in `remote.rs`），持久化在 `{BIFROST_DATA_DIR}/caller-identity.json`，由 `load_or_create_caller_fingerprint` 在首次使用时生成/校验（`is_valid_caller_fingerprint`：必须 `caller-` 前缀 + 32 位 hex）。已不再依赖 username/hostname。 |
| 6 | 向后兼容性 | **不需要**。尚未发布版本，可直接 Breaking Change。 |
| 7 | Grant TTL 策略 | 与 grant_mode 对齐：Once=用完即删，30m/1h/1d=对应 TTL，Permanent=长 TTL + 续期。（已有逻辑，保持不变） |

---

## 八、影响范围评估

### 涉及修改的文件

**Relay Server（TypeScript）：**
- `bifrost-server-v4/app/routes/remoteInvoke.ts` — 删除端点 + 添加 fingerprint 校验
- `bifrost-server-v4/app/service/remoteInvoke.ts` — 校验逻辑 + 移除 client_instance_id 依赖
- `bifrost-server-v4/app/helper/remoteInvokeSse.ts` — 配对审批事件中注入 client 信息

**CLI Caller（Rust）：**
- `crates/bifrost-cli/src/commands/remote.rs` — 主要改造文件（删除 resolve_client_id、新增本地连接文件、简化 CallerRelayClient）
- `crates/bifrost-cli/src/cli.rs` — `Connect` 命令：确认 `--client-id` 不再是必须

**本地 Sync Server（TypeScript）：**
- `packages/bifrost-sync-server/src/routes/remote-invoke.ts` — 同步改造

_实现路径映射（2026-06-16，按当前仓库结构）_：
- `bifrost-server-v4/app/routes/remoteInvoke.ts` → `packages/bifrost-sync-server/src/routes/remote-invoke.ts`
- `bifrost-server-v4/app/service/remoteInvoke.ts` → `packages/bifrost-sync-server/src/remote-invoke/service.ts`
- `bifrost-server-v4/app/helper/remoteInvokeSse.ts` → `packages/bifrost-sync-server/src/remote-invoke/sse.ts`
- 新增（原文未列）：`packages/bifrost-sync-server/src/remote-invoke/ssh-auth.ts`（SSH key 配对路径）、`packages/bifrost-sync-server/src/remote-invoke/cleanup.ts`（pairing / grant 过期清理）、`packages/bifrost-sync-server/src/remote-invoke/types.ts`（共享类型）。
- CLI 侧除 `remote.rs` 外，还实际改动了 `crates/bifrost-cli/src/commands/remote_grant.rs` / `remote_shell.rs` / `remote_ssh_key.rs` / `bifrost_file.rs` / `caller_stream_frame.rs` 以及 `crates/bifrost-admin/src/remote_invoke/**` 一整套模块。本节列表保留原始改造意图，**不再单独刷新**。

### 不涉及修改的文件

- `crates/bifrost-admin/src/remote_invoke/worker.rs` — Client Worker 无需改动
- `crates/bifrost-admin/src/remote_invoke/executor.rs` — 命令执行器无需改动
- `crates/bifrost-admin/src/remote_invoke/types.rs` — 类型定义基本不变
- `crates/bifrost-admin/src/remote_invoke/relay_client.rs` — Client 侧 relay 客户端无需改动

---

## 九、向后兼容性

**不需要向后兼容。** 本项目尚未发布版本，可直接进行 Breaking Change，无需迁移路径。
