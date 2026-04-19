# Remote Invoke 安全重构方案

> 状态：**已确认** | 创建时间：2026-04-20 | 确认：2026-04-20

## 一、问题背景

### 1.1 当前安全漏洞

**核心问题：`GET /v4/remote-invoke/clients` 完全无鉴权，任何人都可以枚举 Relay 上所有在线 Bifrost 实例。**

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
- `grant_id` 本身就是操作凭证（UUID 不可猜测 + 绑定 `caller_fingerprint`）

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
| 窃取 grant_id | 需同时知道 caller_fingerprint（hostname hash）才能使用 |
| 伪造 caller_fingerprint | fingerprint 基于 username+hostname 生成，攻击者需知道目标机器信息 |
| 遍历客户端 | `GET /clients` 已删除，无枚举入口 |
| 重放已过期 grant | Relay 校验 grant 时效（Once/30m/1h/1d/Permanent） |

**结论：** 在 Relay 作为透明中继的场景下，`grant_id` + `caller_fingerprint` 双因子已提供足够安全性，无需引入额外的 `caller_token`。

---

## 四、具体改造点

### 4.1 Relay Server（`bifrost-server-v4`）

#### 4.1.1 删除 `GET /clients` 端点

**当前（`remoteInvoke.ts:334`）：** 任何人可调用，返回所有在线客户端信息。

**改造：** 完全删除此路由，不保留任何访问入口。

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

**校验逻辑伪代码：**
```typescript
function validateGrantOwnership(grant: GrantRecord, callerFingerprint: string): boolean {
  return grant.caller_fingerprint === callerFingerprint;
}
```

#### 4.1.3 `/pairings/start` 改造

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

**Relay 校验逻辑：**
```
1. 查找 grant → 不存在返回 404
2. 校验 grant.caller_fingerprint === 请求中的 caller_fingerprint → 不匹配返回 403
3. 校验 grant.client_instance_id === 请求中的 client_instance_id → 不匹配返回 403
4. 校验 grant 未过期 → 过期返回 403
5. 校验通过 → 创建 call，转发到 Client
```

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

### 4.4 本地 Sync Server（`packages/bifrost-sync-server`）

同步 4.1 的所有改造到本地版本。

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

### Phase 1 — Relay Server 改造（`bifrost-server-v4`）

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

---

## 七、已确认的设计决策

| # | 决策项 | 结论 |
|---|---|---|
| 1 | 本地连接文件位置 | **`{BIFROST_DATA_DIR}/remote-connections.json`**，默认即 `~/.bifrost/remote-connections.json`。完全由启动 bifrost 命令时的数据目录决定。 |
| 2 | `GET /clients` 处理方式 | **完全删除**，不保留任何形式的访问入口。 |
| 3 | `/pairings/start` 是否接受 `client_instance_id` | **完全移除**，只通过 pair_code 解析。Caller 不应在配对前知道 client_id。 |
| 4 | 重复配对（同 Caller 对同 Client 再次 connect） | 新 grant 与旧 grant 并存，本地连接文件更新为最新 grant，旧 grant 在服务端自然过期。 |
| 5 | `caller_fingerprint` 生成算法 | 暂保持现有 `simple_hash`，足够做客户端区分。后续如有需要可平滑升级到 SHA-256。 |
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

### 不涉及修改的文件

- `crates/bifrost-admin/src/remote_invoke/worker.rs` — Client Worker 无需改动
- `crates/bifrost-admin/src/remote_invoke/executor.rs` — 命令执行器无需改动
- `crates/bifrost-admin/src/remote_invoke/types.rs` — 类型定义基本不变
- `crates/bifrost-admin/src/remote_invoke/relay_client.rs` — Client 侧 relay 客户端无需改动

---

## 九、向后兼容性

**不需要向后兼容。** 本项目尚未发布版本，可直接进行 Breaking Change，无需迁移路径。
