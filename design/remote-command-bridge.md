# 远程调用桥接方案

## 背景

- 目标是在 Bifrost 现有本地管理端基础上，新增“远程调用 Bifrost 客户端命令”的能力。
- 中转服务按两个阶段落地：
  - **本地验证阶段**：先在 `packages/bifrost-sync-server` 内实现远程调用 relay，方便本地完成完整闭环测试
  - **云端部署阶段**：在本地验证通过后，再迁移到 `bifrost-server-v4/app`，部署到 TCE 做组中测试
- 统一协议模型保持一致：
  - 调用方 -> 云端：HTTP + SSE
  - 云端 -> Bifrost 客户端：SSE 长连接
  - Bifrost 客户端 -> 云端：HTTP 上行事件接口
- 本地 Bifrost WebUI 需要承载授权、记录查询、事件通知和历史审计能力。

## 设计结论

### 核心判断

- `6` 位随机码只能作为“配对引导信息”，不能作为最终安全凭据。
- 原因很直接：
  - 熵只有 `10^6`，天然可暴力猜测。
  - 云端链路存在重放、撞库和枚举风险。
  - 一旦把它当成最终鉴权材料，就无法支撑“永久授权”“限时授权”“吊销后失效”“端到端加密绑定”等需求。

### 最终分层

方案拆成四层，职责必须分离：

1. **配对层（可见性管控）**
  - Relay 绝不主动暴露注册的客户端信息。`pair_code` 是唯一的客户端发现机制。
  - Bifrost 客户端在发现模式下生成 `6` 位一次性授权码，仅用于让调用方证明“知道当前可见的本地码”。
  - Caller 不需要注册、不需要登录、不需要 token。
2. **授权层（Client 主动授权）**
   - 真正是否允许执行命令，必须由本地 Bifrost 用户在 WebUI 中人工批准。
   - 只有被调用客户端审批通过后，才会向 Caller 释放 `client_instance_id`、设备信息以及 `grant`。
3. **凭证层（grant 绑定）**
   - 授权成功后，`grant_id`（UUID 不可猜测）+ `caller_fingerprint`（主机标识 hash）构成操作凭证。
   - 每次创建调用时，Relay 签发 per-call 的 `relay_token`，仅用于该次调用的路由和 SSE 订阅。
   - Caller 不需要任何身份 token（无 `x-bifrost-token`），安全性由可见性管控 + grant 绑定保障。
4. **加密层**（已实现）
   - 调用内容不依赖云端 token 明文传输，而是通过调用方与 Bifrost 客户端之间的 X25519 ECDH 密钥协商 + HKDF-SHA256 派生 + ChaCha20-Poly1305 / AES-256-GCM AEAD 实现双向加密。

这四层同时存在，才能同时满足“pair_code 门控发现”“需要人工授权”“grant 绑定可复用”“云端不知道明文内容”。

## 角色与边界

### 角色

- **调用方**
  - 远端发起命令的用户或系统。
- **Relay**
  - 本地验证阶段对应 `packages/bifrost-sync-server`
  - 云端部署阶段对应 `bifrost-server-v4/app`
  - 只负责会话编排、在线路由、授权状态协调、审计落库、SSE/HTTP 转发。
- **Bifrost 客户端**
  - 真正执行命令的一端。
  - 负责展示一次性授权码、接收授权决策、解密指令、执行命令、回传结果摘要。
- **本地 WebUI**
  - Bifrost 用户交互界面。
  - 负责授权、查看历史、撤销授权、查看事件。

### 信任边界

- 云端 Relay **是透明中继服务，不管理 Caller 身份**（Caller 无需任何鉴权 token）。
- Relay 的安全模型基于两根支柱：
  1. **客户端可见性管控** — Relay 绝不主动暴露注册的客户端信息。`pair_code` 是唯一的客户端发现机制。
  2. **Client 主动授权** — 只有被调用客户端审批通过后，才会向 Caller 释放 `client_instance_id`、设备信息以及 `grant`。
- Caller 不需要注册、不需要登录、不需要 token。
- Client 注册仍需 `client_auth_token`（保持现有机制不变）。
- 云端只知道：
  - 哪个调用会话在进行
  - 哪个客户端在线
  - 哪个授权策略生效
  - 哪些摘要和审计事件需要保留

### grant 作为操作凭证的安全性

无 `caller_token` 时，操作鉴权完全靠 `grant_id` + `caller_fingerprint`：

| 攻击场景 | 防御 |
|---|---|
| 猜测 grant_id | UUID v4，128 位随机，不可暴力枚举 |
| 窃取 grant_id | 需同时知道 caller_fingerprint（hostname hash）才能使用 |
| 伪造 caller_fingerprint | fingerprint 基于 username+hostname 生成，攻击者需知道目标机器信息 |
| 遍历客户端 | `GET /clients` 已删除，无枚举入口 |
| 重放已过期 grant | Relay 校验 grant 时效（Once/30m/1h/1d/Permanent） |

## 目标与非目标

### 目标

- 支持通过云端中转远程调用 Bifrost 客户端命令。
- **首版（已扩展，2026-06-16）** 命令范围已超出最初的「仅查询」边界：除只读查询命令白名单外，还实现了 `RemoteShellExec`、`RemoteShellInteractive`、`RemotePowerMgmt`、`RemoteImGateway` 等 `GrantScope`，并叠加独立的 `FileAccessScope`（含远端文件 read/list/stat/glob/find/hash/write/edit/patch/mkdir/move/delete）。受 `FileAccessPolicy` 与 GrantScope 双重门控。
- 支持大流量结果传输与大载荷分片传输，不把协议限制在“小请求/小响应”场景。
- 支持多个 Bifrost 远端调用客户端同时在线，并在 `Settings -> Remote Invoke` 中统一管理。
- 支持多个远程调用方（不同 `caller_fingerprint`）对同一 Bifrost 客户端并发发起调用，各调用方授权独立、会话隔离。
- 支持人工授权策略：
  - 一次调用
  - 多次调用 `30 分钟`
  - 多次调用 `1 小时`
  - 多次调用 `1 天`
  - 永久
- 支持授权后签发 per-call `relay_token`，用于单次调用的路由与 SSE 订阅。
- 支持 WebUI 中查看完整调用记录、事件、来源、命令、内容摘要。
- 调用记录保留 `90` 天，且最多保留 `10k` 条。

### 非目标

- ~~首版不支持任意 shell~~ — 已通过 `RemoteShellExec` / `RemoteShellInteractive` GrantScope 开放，但仍要求每次调用都由 Client 端审批人为授权，并保留命令白名单与 FileAccessPolicy 限制。
- 首版不支持配置修改、状态变更、文件读写、脚本执行、证书/规则/values 等管理写操作。
- 首版不做任意文件上传/下载隧道。
- 首版不做多客户端共享一个活跃终端会话。
- 首版不做全功能 shell TTY 仿真，优先支持“命令执行 + 流式输出”模型。
- 首版不把原始明文命令结果持久化到云端数据库。

## 总体架构

### 分阶段落地

#### Phase 0：本地 Relay 试验田

- 首先在 `packages/bifrost-sync-server` 内新增远程调用 relay 能力。
- 目标不是一步到位替代 `bifrost-server-v4`，而是先满足：
  - 本地单机启动简单
  - 调试成本低
  - API/协议迭代快
  - 方便配合本地 Bifrost 客户端做端到端验证
- 本阶段验收标准：
  - 发现模式 -> 一次性授权码 -> 人工授权 -> token -> 端到端加密 -> 查询结果回传 整条链路可跑通
  - `bifrost remote` 可直接对本地 relay 发起调用
  - 审计记录可查
  - 断线重连、一次授权、限时授权至少覆盖主路径

#### Phase 1：迁移到云端部署版

- 当 `packages/bifrost-sync-server` 跑通后，再把同一套协议和状态机迁移到 `bifrost-server-v4/app`。
- 云端迁移阶段主要关注：
  - 数据模型迁移
  - 鉴权与用户体系接入
  - TCE 部署适配
  - 组内联调稳定性

#### 设计原则

- **协议先行，宿主后置**：
  - 先把 relay 协议在 `bifrost-sync-server` 定型
  - 再把相同协议移植到 `bifrost-server-v4`
- **本地版优先简单可测**：
  - 优先使用原生 HTTP + SSE
  - 优先 SQLite
  - 优先密码/测试 token 鉴权
- **云端版优先接入现有体系**：
  - 接入 `bifrost-server-v4` 的 session / user / SSO / deployment 体系

### 逻辑链路

1. Bifrost 客户端启动远程调用模块，向 Relay 注册并建立 SSE 长连接。
2. 客户端在本地 WebUI 的 Remote Invoke Tab 里进入发现模式，生成一个一次性 `6` 位授权码。
3. 调用方（Caller）通过带外方式获取 `pair_code`（微信/口头/截屏），通过 Relay 发起配对请求。
   - Caller 只需提供 `pair_code` + `caller_info`，**不需要提前知道 `client_instance_id`**。
   - Relay 从 `pair_code` 自动解析出目标客户端。
   - 如果 Relay 前层临时触发 `503 overload-protect`，CLI 会做有限次退避重试；若仍失败，需输出明确提示，指引用户稍后重新执行 `bifrost remote connect <pair-code>`，而不是直接暴露基础设施原始报错。
4. Relay 把“待授权远程调用请求”推送给对应 Bifrost 客户端。
5. 本地 WebUI 全局弹窗展示请求信息，用户选择授权策略。
6. 授权通过后：
   - Relay 创建 `grant`（绑定 `caller_fingerprint`）
   - 通过 SSE 将 `{grant_id, client_instance_id, device_name, platform, grant_mode}` 返回给 Caller
   - Caller 将连接信息保存到本地 `{BIFROST_DATA_DIR}/remote-connections.json`
7. 后续命令执行时：
   - Caller 从本地连接文件读取 `client_instance_id` + `grant_id` + `caller_fingerprint`
   - 向 Relay 验证 grant 有效性
   - 通过 `POST /calls/open` 创建调用（Relay 签发 per-call `relay_token`）
   - 通过 SSE 持续接收事件流
   - 客户端通过 SSE 接收下行事件，通过 HTTP 回传输出帧
8. 会话结束后：
   - 客户端上报执行结果摘要
   - Relay 持久化记录与事件
   - WebUI 可查询详情

### 本地测试拓扑

- 本地推荐同时启动：
  - `packages/bifrost-sync-server`：本地 relay
  - 本地 Bifrost 客户端：执行端 + WebUI 授权端
  - 调用方 CLI：`bifrost remote ...`
- 目标拓扑：
  - `bifrost remote` -> `bifrost-sync-server`
  - `bifrost-sync-server` -> 本地 Bifrost 客户端（SSE 下行）
  - 本地 Bifrost 客户端 -> `bifrost-sync-server`（HTTP 上行）
- 这样在单机环境就能验证完整交互，而不必先依赖 TCE 和 `bifrost-server-v4` 的部署链路。

### 传输形态

- **调用方 <-> Relay**
  - `POST` 创建调用
  - `GET` SSE 订阅结果流
  - `POST` 发送追加输入 / 取消 / 心跳
- **Relay <-> 客户端**
  - 单条 SSE 持久下行连接
  - 多个 HTTP 上行接口承载心跳、授权决策、结果回传
  - 通过 `client_instance_id` + `stream_id` 关联同一客户端会话
- **本地 WebUI <-> 本地 Bifrost**
  - 复用现有 admin API + push/notification 机制

### 为什么不用 WebSocket

- 当前基础设施对 WebSocket 的支持度和稳定性不足，不作为首版依赖。
- `SSE + HTTP POST` 更贴近现有基础设施能力：
  - 下行事件使用 SSE，天然适合服务端持续推送
  - 上行动作使用普通 HTTP，更易复用鉴权、网关、日志和重试链路
- 需要注意：SSE 不是双向协议，因此客户端到 Relay 的消息必须拆成独立 HTTP 接口。

## 关键安全设计

### 1. 六位码只做配对引导

- 授权码只在客户端发现模式开启时生成，单个授权码有效期 `2` 分钟。
- 单次生成后只允许成功消费一次。
- **超时自动轮换**：授权码过期后，若发现模式仍然开启，客户端自动生成新的 6 位授权码并向 Relay 重新注册，旧码立即作废。WebUI 实时刷新展示新码和重置后的倒计时。
- 发现模式在以下任一条件满足后自动关闭：
  - 授权码被成功消费（配对成功）
  - 用户手动关闭发现模式
- 不写入持久库，只保存在：
  - 客户端内存
  - Relay Redis 临时状态
- 安全约束：
  - 同一客户端最多同时存在 `1` 个活跃配对码
  - 同一调用方 IP 对同一客户端 `5` 分钟内最多尝试 `5` 次
  - **全局限流**：同一 `client_instance_id` `5` 分钟内全局最多 `10` 次验证尝试（不区分调用方 IP），防止分布式爆破
  - 连续失败触发冷却
  - **pair_code 校验必须使用 constant-time comparison**（如 `subtle::ConstantTimeEq`），防止时序侧信道泄露前缀匹配信息

### 2. 人工授权才是真正的执行许可

- 配对成功只代表“这个远端知道当前一次性授权码”。
- 只有本地用户在 WebUI 中点击批准，调用才进入可执行状态。
- 未经人工批准，调用方不能拿到后续 `relay token`。
- client 侧所有上行审批/执行事件都必须绑定到认证后的 `client_instance_id`：
  - `POST /v4/remote-invoke/client/grants/:pairingId/decision` 必须校验 pairing 的目标 client 与当前 `client_auth_token` 对应 client 一致
  - `POST /v4/remote-invoke/client/calls/:callId/frame|exit` 必须校验 call 的归属 client 与当前 `client_auth_token` 对应 client 一致
  - 禁止“任意已注册 client 只要知道 `pairing_id` / `call_id` 就能代替目标 client 操作”的旁路

### 3. relay_token 只负责 per-call 路由

- **`relay_token` 是 per-call 级别的临时路由凭据**，每次创建调用（`POST /calls/open`）时由 Relay 签发。
- 使用 **随机 256 bit opaque token**（64 hex 字符），不使用 JWT。
- token 用途：
  - 调用方订阅 `GET /calls/:call_id/events` SSE
  - 调用方发送 `POST /calls/:call_id/input` 输入帧
  - 调用方取消 `POST /calls/:call_id/cancel`
- token 不是 Caller 的身份凭据——Caller 无需任何身份 token。
- **真正的操作凭证是 `grant_id` + `caller_fingerprint`**：
  - `grant_id`：UUID v4，不可猜测，绑定 `caller_fingerprint`
  - `caller_fingerprint`：基于调用方主机信息生成的稳定标识
  - 所有涉及 grant 的 Caller 端点都需要同时提供两者，Relay 校验绑定关系

### 3.1 授权复用与本地连接文件

- 一次配对 + 人工授权完成后，连接信息保存到 Caller 本地文件，用于后续 `bifrost remote` 直接复用。
- **本地连接文件路径**：`{BIFROST_DATA_DIR}/remote-connections.json`（默认 `~/.bifrost/remote-connections.json`）
- **文件结构**：
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
- 不再有 `caller_token` 字段。`grant_id` + `caller_fingerprint` 就是操作凭证。
- **授权复用流程**：
  - `bifrost remote` 在发起新命令前，从本地连接文件读取 `client_instance_id` + `grant_id` + `caller_fingerprint`
  - 调用 Relay 的 `GET /grants/reusable?client_instance_id=X&caller_fingerprint=Y` 验证 grant 仍有效
  - 如果有效，直接进入 `POST /calls/open` 流程
  - 如果无效（过期/撤销），报错 "authorization expired, please run `bifrost remote connect <pair-code>` again"
- **重复配对处理**：同一 Caller 对同一 Client 再次 `connect` 时，新 grant 与旧 grant 并存，本地连接文件更新为最新 grant，旧 grant 在服务端自然过期。

### 4. 真正保密依赖端到端加密（已实现）

> **实现状态（2026-06-16）**：X25519 ECDH + HKDF-SHA256 + ChaCha20-Poly1305 / AES-256-GCM 已在 `crates/bifrost-admin/src/remote_invoke/types.rs`（`EncryptedEnvelope` v2、`derive_open_call_session_key`、`derive_call_session_key`）和 `crates/bifrost-cli/src/commands/remote.rs` 实现，`EncryptedEnvelope` 真正承载密文。本地 caller 侧的 shared_secret 经 `remote-connections.key` 派生密钥使用 AES-256-GCM 加密落盘（`shared_secret_encrypted` 字段）。

- 授权通过后，由调用方与客户端交换临时公钥，推荐：
  - `X25519` 做 ECDH
  - `HKDF-SHA256` 派生会话密钥
  - `ChaCha20-Poly1305` 或 `AES-256-GCM` 做 AEAD
- 派生材料：
  - `shared_secret`（X25519 ECDH 输出）
  - `relay_token`
  - `call_id`
  - `caller_ephemeral_pub`
  - `client_ephemeral_pub`
  - `sha256(pair_code)`（Relay 不可控的 binding 材料，防止 Relay MITM 替换公钥）
- 结果：
  - `caller_to_client_key`
  - `client_to_caller_key`
  - `session_id`
- 云端只转发密文包，无法看到命令明文和返回明文。

#### 4.1 Relay MITM 防护

- **核心威胁**：Relay 转发双方临时公钥时可替换为自己生成的密钥对，做经典中间人攻击。`relay_token` 由 Relay 签发，攻击者控制 Relay 后可同时伪造 token，因此仅靠 token 做 channel binding 不够。
- **防护措施**：
  1. **引入 Relay 不可控的 binding 材料**：将 `sha256(pair_code)` 加入 HKDF 输入。`pair_code` 由客户端本地生成，调用方通过带外方式获取，Relay 即使截获也无法在替换公钥后让双方派生出一致的会话密钥。
  2. **Caller ephemeral signature**：Caller 用自己的 PoP Ed25519 长期私钥签名本次 `caller_ephemeral_pub`，签名 payload 绑定 relay-derived `caller_fingerprint`（即 caller PoP 公钥 SPKI DER 的 SHA-256 hex）。Relay 必须在 SSE `pairing_request` 与 pending-pairings polling 响应中原样转发 `caller_ephemeral_sig`；Client 若看到 caller 长期公钥但缺失或验证失败的签名，必须 fail-closed 拒绝审批。
  3. **Session Fingerprint 验证**：密钥协商完成后，双方各自计算 session fingerprint：
     - `fingerprint = sha256(caller_ephemeral_pub || client_ephemeral_pub || session_id)`
     - 客户端在 WebUI 的授权详情中展示此 fingerprint（截短为 8 位 hex）
     - 调用方 CLI 输出同样的 fingerprint
     - 用户可在高安全模式下人工比对（类似 Signal Safety Number）
  4. **对于可复用授权的后续调用**：每次新 call 重新生成 ephemeral key pair 并协商新的 per-call 会话密钥，将 `grant_id` + `call_id` 加入 HKDF 输入，确保即使某次会话密钥泄露也不影响其他调用。

#### 4.2 Per-Call 密钥派生

- 即使授权为永久授权或限时授权，**每次新 call 必须重新生成 ephemeral key pair 并派生新的会话密钥**。
- 禁止多次 call 复用同一套会话密钥。
- per-call 密钥派生输入：
  - `shared_secret`（本次 ECDH）
  - `grant_id`
  - `call_id`
  - `caller_ephemeral_pub`（本次）
  - `client_ephemeral_pub`（本次）
- ephemeral key 的生命周期严格为 per-call：call 结束后立即从内存清除。

#### 4.3 Nonce 管理

- AEAD 加密使用 **counter-based nonce**，禁止随机 nonce（避免 AES-256-GCM 的 nonce 重用灾难）。
- nonce 格式（12 字节）：
  - `direction_byte (1B) || zero_padding (3B) || seq_u64 (8B)`
  - `direction_byte`：`0x01` = caller_to_client，`0x02` = client_to_caller
- 双方使用不同的 direction byte 确保 nonce 空间完全隔离。
- 每个方向的 seq 从 0 单调递增，接收方必须拒绝 seq 回退或重复的帧。
- 接收方必须先校验 wire payload 里的 nonce 与外层 `direction + seq` 精确匹配，再解密并更新 replay window；外层 `seq` 不能单独作为去重依据，避免篡改外层序号绕过重放保护或污染 replay window。
- `call_exit` 的 no-AAD 加密载荷固定保留 `seq = 0` 的 counter nonce，普通 stream frame 使用其外层 `seq` 对应的 counter nonce。
- 如果使用 `ChaCha20-Poly1305`（24 字节 nonce / XChaCha20），可改用 `direction_byte (1B) || zero_padding (15B) || seq_u64 (8B)`。

### 4.4 客户端身份认证（client_auth_token）

- **client_auth_token 是客户端与 Relay 之间的身份凭据**，用于认证客户端 SSE 连接和所有客户端上行 HTTP 请求。
- **签发流程**（当前实现）：
  1. 客户端首次启动并生成 `client_instance_id` 时，同时生成 `client_long_term_pubkey`。
  2. 客户端向 Relay 的 `POST /v4/remote-invoke/client/register` 提交注册请求，携带 `client_instance_id`、`client_long_term_pubkey`、`device_name`、`platform`、`signature`。
  3. Relay 签发 `client_auth_token`（随机 256 bit opaque token = 64 hex 字符），绑定 `client_instance_id` + `pubkey`。
  4. token 存储在 Redis 中，TTL 30 天。
- **校验规则**：
  - 客户端所有上行请求（SSE 建连、心跳、授权决策、帧回传等）必须携带 `client_auth_token`（通过 `Authorization: Bearer` header 或 query 参数）。
  - Relay 通过 `requireClientAuth()` 中间件校验 token 有效性，并比对 `client_instance_id` 是否匹配。
  - 使用常量时间比对（constant-time comparison）防止时序侧信道攻击。
- **轮换策略**（当前实现）：
  - token 有效期 `30 天`。
  - 客户端检测到 token 被拒绝（`401`）后，自动触发完整的重新注册流程（而非续签）。
  - **Token 续签端点**（`POST /client/token/renew`）**尚未实现**，当前通过重新注册替代。

> **设计文档 vs 实现差异（2026-06-16）**：Ed25519 challenge/response 签名验证已实现（见 `packages/bifrost-sync-server/src/remote-invoke/service.ts` 中 `algorithm: ed25519` 验签逻辑与 `invalid_registration_signature` 错误码）；独立 token 续签端点 `POST /client/token/renew` 仍未实现（planned, not yet shipped as of 2026-06-16），当前依赖客户端在收到 `401` 后通过重新注册（challenge + register）替代续签。

### 5. 审计摘要与原文分离

- 为满足“查看调用的指令、传输内容摘要”等需求，首版采用“**红线外摘要**”方案：
  - 调用方在发起请求时上送一个**脱敏摘要**，例如：
    - 可执行文件名
    - 参数预览（敏感参数掩码）
    - 原始载荷 `sha256`
    - 原始长度
  - 客户端在执行后补充：
    - 实际执行命令摘要
    - 输出摘要
    - 退出码
    - 耗时
- 云端只持久化摘要，不持久化原文。
- 这样可以兼顾：
  - 用户可审计
  - 云端不可见明文

## 会话模型

### 实体

1. **Client Presence**
   - 客户端在线实例信息
2. **Pairing Session**
  - 一次性授权码与待授权请求
3. **Authorization Grant**
   - 一次性 / 限时 / 永久授权策略
4. **Call Session**
   - 某一次真实命令调用
5. **Call Event**
   - 调用过程中的事件流水

### 状态机

#### Pairing Session

- `created`
- `code_verified`
- `pending_approval`
- `approved`
- `rejected`
- `expired`
- `cancelled`

#### Authorization Grant

- `active`
- `expired`
- `revoked`
- `consumed`（一次调用型）
- `removed`（管理端手动删除）

#### Call Session

- `pending`
- `authorized`
- `key_exchanged`
- `streaming`
- `completed`
- `failed`
- `cancelled`
- `timeout`

## 详细流程

### A. 客户端上线

1. Bifrost 客户端启动后与 Relay 建立 `/v4/remote-invoke/client/stream`。
2. 连接时上送：
   - `client_instance_id`
   - `device_name`
   - `platform`
   - `bifrost_version`
   - `local_account_id` 或绑定用户标识
   - `client_long_term_pubkey`
3. Relay 记录 presence 到 Redis，并周期心跳保活。

### B. 发现模式与一次性授权码

1. 本地用户在 WebUI 打开 Remote Invoke Tab。
2. 点击“进入发现模式”。
3. 客户端本地生成：
   - `pair_code`：6 位数字
   - `pairing_id`
   - `expires_at`
   - `discovery_session_id`
4. 客户端通过 HTTP 向 Relay 注册这组临时映射，并把当前客户端标记为 `discoverable`。
5. WebUI 展示：
   - 一次性授权码
   - 剩余有效时间
   - 当前客户端名称
   - 发现模式状态
6. 授权码一旦被成功消费，或手动关闭发现模式后，当前 `discovery_session_id` 立即失效。
7. 若授权码超时未被消费且发现模式仍开启，客户端自动生成新 `pair_code` 和 `pairing_id`，沿用当前 `discovery_session_id`，向 Relay 注册新映射并废弃旧码。WebUI 无缝刷新展示新授权码。

### C. 调用方发起配对（`bifrost remote connect`）

1. 调用方通过带外方式获取 `pair_code`（Client 用户通过微信/口头/截屏等方式告知）。
2. 向 Relay 发起 `POST /v4/remote-invoke/pairings/start`。
3. 请求只需包含：
   - `pair_code`（6 位一次性授权码）
   - `caller_info`
     - `fingerprint`（调用方设备指纹）
     - `display_name`（调用方设备名）
4. **不需要 `client_instance_id`** — Relay 通过 `pair_code` 自动解析出目标客户端。
5. **不需要 `command` / `command_summary`** — connect 阶段只做配对授权，不携带具体命令。
6. Relay 验证授权码后创建 `pairing session`，状态变为 `pending_approval`。
7. Relay 把待授权事件推送给客户端。

### C.1 调用方执行命令

1. 从本地连接文件（`{BIFROST_DATA_DIR}/remote-connections.json`）读取已保存的连接信息。
2. 通过 `resolve_local_connection()` 从本地文件中按前缀匹配 `client_instance_id`：
   - 0 个匹配 → 报错 "no saved connection, please run `bifrost remote connect <pair-code>` first"
   - 1 个匹配 → 直接使用
   - 多个匹配 → 交互选择（展示 `device_name + short_id`）
3. 调用 `GET /grants/reusable?client_instance_id=X&caller_fingerprint=Y` 验证 grant 有效性。
4. 有效 → 通过 `POST /calls/open` 创建调用并执行命令。
5. 无效 → 报错 "authorization expired, please run `bifrost remote connect <pair-code>` again"。

### D. 本地人工授权

1. 客户端收到待授权事件后：
   - 写入本地 admin notification
   - 触发全局弹窗
   - Remote Invoke Tab 展示 pending item
2. 弹窗展示：
   - 调用方设备指纹（`caller_fingerprint`，基于 username+hostname 的 hash，展示为截短格式）
   - 调用方显示名
   - 远端来源 IP / 地域（如果可得）
   - User-Agent
   - 命令摘要
   - 首次见到时间
   - 是否为首次配对的设备（若 `caller_fingerprint` 在历史授权中无记录，醒目标注"⚠️ 新设备"）
   - 风险提示
3. 用户可选择：
   - 拒绝
   - 仅本次
   - `30m`
   - `1h`
   - `1d`
   - 永久
4. 若批准，客户端通过 HTTP 向 Relay 提交 grant 创建请求。

**授权操作安全约束**：
- 所有写操作（`SubmitGrantDecision`、grant 创建、grant 撤销）必须携带 **CSRF token**，防止恶意页面通过跨站请求自动触发授权。
  - CSRF token 由客户端 WebUI 在页面加载时从本地 admin 接口获取，嵌入表单/请求 header。
  - 客户端校验 token 后才处理授权请求。
- **永久授权的二次确认**：选择「永久」时，WebUI 必须弹出二次确认对话框，要求用户输入 `CONFIRM` 文本后才提交，防止误触导致过度授权。

### E. 配对审批通过后

1. Client 用户在 WebUI 中批准配对，选择授权策略（仅本次 / 30m / 1h / 1d / 永久）。
2. Client 通过 HTTP 向 Relay 提交 grant 创建请求（`POST /client/grants/:pairingId/decision`）。
3. Relay 创建 `authorization_grant`，绑定 `caller_fingerprint`。
4. Relay 通过 pairing watcher SSE 向 Caller 推送 `decision` 事件：
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
5. Caller 从此事件中获得 `client_instance_id`、`grant_id`、`device_name`、`platform`、`grant_mode`。
6. Caller 将连接信息保存到本地 `{BIFROST_DATA_DIR}/remote-connections.json`。
   - 如果同一个 `{client_instance_id, relay_url}` 已存在，更新（覆盖旧 grant）。

### E.1 命令执行（调用创建）

1. Caller 使用本地连接文件中的 `grant_id` + `caller_fingerprint` 调用 `POST /calls/open`。
2. Relay 校验：
   - grant 存在且状态为 `active`
   - `grant.caller_fingerprint === 请求中的 caller_fingerprint`
   - `grant.client_instance_id === 请求中的 client_instance_id`
   - grant 未过期
   - `remaining_calls > 0`（对于 `once` 类型）
3. 校验通过后，Relay 签发 per-call 的 `relay_token`（随机 256 bit），创建 call。
4. Relay 通过客户端 SSE 推送 `call_open` 事件给 Client。
5. Client 执行命令，通过 HTTP 回传输出帧。

### F. 调用执行与流式传输

1. 调用方使用 `POST /v4/remote-invoke/calls/:call_id/input`
   - body 为加密信封
   - 也可支持首次创建时直接附带首个命令帧
2. 调用方使用 `GET /v4/remote-invoke/calls/:call_id/events`（通过 `Authorization: Bearer <relay_token>` header 认证）
   - 以 SSE 接收：
     - `status`
     - `stdout`
     - `stderr`
     - `progress`
     - `exit`
     - `heartbeat`
     - `error`
3. Relay 通过客户端 SSE 流将密文帧下发给客户端。
4. 客户端解密后执行命令，并通过 HTTP 接口把输出切块加密后回传。
5. Relay 把加密结果作为 SSE data 原样下发给调用方。

### F.1 大流量传输设计

- 首版必须支持大结果集和大载荷传输，不能假设所有查询结果都很小。
- 设计要求：
  - 调用输入与输出都按分片帧传输
  - 单帧建议控制在 `64 KiB ~ 256 KiB`
  - 每帧使用 `seq` 保证顺序，用 `direction` 区分方向
  - Relay 不聚合整条明文结果，只做分片转发、顺序缓存和限流
- 对大结果流的约束：
  - 客户端执行器支持边执行边产出分片
  - 调用方支持边接收边消费 SSE `frame`
  - Relay 具备背压能力，避免单连接无限制占用内存
- 可靠性要求：
  - SSE 断线后可通过 `Last-Event-ID` / `cursor` 续流
  - Relay 对未确认的分片保留短期游标缓存
  - 必要时可把超大临时密文分片落到临时文件，而不是常驻内存

### G. 会话结束

1. 客户端通过 HTTP 回传：
   - `exit_code`
   - `duration_ms`
   - `stdout_digest`
   - `stderr_digest`
   - `bytes_in/out`
2. Relay 更新 `call session` 为终态。
3. 若授权为“一次调用”，同步把 grant 标记为 `consumed`。

## 协议设计

### 调用方 -> Relay HTTP API

#### 0. 查询可复用授权

- `GET /v4/remote-invoke/grants/reusable`
- **无需身份认证**（Caller 不需要 token）
- Query:
  - `client_instance_id`
  - `caller_fingerprint`
- Relay 校验：只返回 `caller_fingerprint` 完全匹配的 grant
- 响应：
  - 是否存在可复用授权
  - `grant_id`
  - `grant_mode`
  - `expires_at`
  - `status`

#### 1. 创建配对请求

- `POST /v4/remote-invoke/pairings/start`
- **无需身份认证**（Caller 不需要 token）
- 请求：
  - `pair_code`（Relay 从 pair_code 自动解析 `client_instance_id`）
  - `caller_info`（`fingerprint` + `display_name`）
- **不再需要**：`client_instance_id`、`caller_pubkey`、`command_summary`、`command`
- 响应：
  - `pairing_id`
  - `status=pending_approval`
  - `approval_sse_url`

#### 1.1 创建调用

- `POST /v4/remote-invoke/calls/open`
- **无需身份认证**（通过 grant 绑定校验替代）
- 请求：
  - `grant_id`
  - `client_instance_id`
  - `caller_fingerprint`
  - `command`（`{ "command": "status", "args_json": null }`）
  - `command_summary`（`{ "command_preview": "status" }`）
- Relay 校验：
  - grant 存在 + `caller_fingerprint` 匹配 + `client_instance_id` 匹配 + 未过期
- 响应：
  - `call_id`
  - `relay_token`（per-call 临时 token）

#### 2. 等待授权状态

- `GET /v4/remote-invoke/pairings/:pairing_id/watch`
- **无需身份认证**（`pairing_id` 是一次性 UUID）
- SSE 事件：
  - `status`（包含 `pending_approval`、`timeout` 等）
  - `decision`（审批结果）
- `decision` 中 `status=approved` 时返回：
  - `grant_id`
  - `client_instance_id`
  - `device_name`
  - `platform`
  - `grant_mode`

#### 3. 发送调用输入

- `POST /v4/remote-invoke/calls/:call_id/input`
- Header:
  - `Authorization: Bearer <relay_token>`
- Body:
  - `EncryptedEnvelope`

#### 4. 订阅调用输出

- `GET /v4/remote-invoke/calls/:call_id/events`
- Header:
  - `Authorization: Bearer <relay_token>`
- **安全约束**：token 必须通过 `Authorization` header 传递，**禁止放在 URL query string 中**（避免 access log、代理日志、Referer header 泄露 token）。SSE 客户端使用 `fetch` API 或支持自定义 header 的 EventSource polyfill。
- SSE 事件：
  - `status`
  - `frame`
  - `exit`
  - `error`
  - `heartbeat`

#### 5. 取消调用

- `POST /v4/remote-invoke/calls/:call_id/cancel`

#### ~~6. 列出当前用户的授权记录~~ — 已删除

> Caller 无需列出所有 grant。仅通过 `/grants/reusable` 查询具体授权即可。

#### ~~6.1 列出当前用户在线客户端~~ — 已删除

> `GET /v4/remote-invoke/clients` 已**完全删除**，不保留任何访问入口。客户端发现仅通过 `pair_code` 机制。

#### ~~7. 更新授权有效期~~ — 已删除

> Grant 属性由 Client 审批时决定，Caller 不应修改。Grant 管理由 Client 侧通过 WebUI 完成。

#### 8. 移除授权

- `DELETE /v4/remote-invoke/grants/:grant_id`
- **无需身份认证**（通过 `caller_fingerprint` 校验归属替代）
- Query:
  - `caller_fingerprint`
- Relay 校验：`grant.caller_fingerprint === 请求参数中的 caller_fingerprint`

### Relay -> 客户端 SSE 事件

#### 客户端订阅

- `GET /v4/remote-invoke/client/stream`
- Header / Query:
  - `client_instance_id`
  - `client_auth_token`
  - `stream_id`

#### 下行事件类型

- `client_hello_ack`
- `pairing_request`
- `grant_created`
- `call_open`
- `call_frame`
- `call_cancel`
- `grant_revoked`
- `ping`

#### 下行事件职责边界

- `grant_created`
  - 语义：配对审批通过后，将新授权同步到 client 本地缓存
  - 最小字段：`grant_id`、`caller_fingerprint`、`grant_mode`
  - 可选字段：`caller_info.display_name`、`expires_at`
  - **不承载调用执行语义**，不要求 `call_id`、`command`
- `call_open`
  - 语义：在已有 grant 基础上发起一条具体调用
  - 必须包含：`call_id`、`grant_id`、`command`
  - client 必须先校验 `grant_id` 已存在于本地 `local_grants` 且状态有效，再执行命令
- 两类事件禁止混用：
  - 不允许把 `grant_created` 当成 `call_open` 的替代事件
  - 不允许在 client 侧要求 `grant_created` 提供 `call_id` / `command`

### 客户端 -> Relay HTTP API

#### 0. 客户端注册

- `POST /v4/remote-invoke/client/register/challenge`
- Header:
  - `x-bifrost-token`（必须，由 client 所属用户的 sync session 提供）
- Body:
  - `client_instance_id`
- 响应：
  - `challenge_id`
  - `challenge`
  - `expires_at`
  - `algorithm = "ed25519"`

- `POST /v4/remote-invoke/client/register`
- Header:
  - `x-bifrost-token`（必须，与 challenge 申请阶段保持同一用户上下文）
- Body:
  - `challenge_id`
  - `client_instance_id`
  - `client_long_term_pubkey`
  - `device_name`
  - `platform`
  - `bifrost_version`
  - `signature`（使用 `client_long_term_privkey` 对 `["bifrost-remote-register-v1", challenge_id, challenge, client_instance_id, device_name, platform, bifrost_version, client_long_term_pubkey, timestamp]` 的 Ed25519 签名）
  - `timestamp`
- 响应：
  - `client_auth_token`
  - `expires_at`
- 安全约束：
  - Relay 先校验 `x-bifrost-token`，确认注册请求归属某个已登录用户
  - Relay 只接受一次性 challenge，challenge 过期或被消费后必须拒绝
  - Relay 验证签名合法后才签发 token，确保注册方真正持有长期私钥
  - 同一 `client_instance_id` 重复注册时，Relay 验证 owner 与 pubkey 一致性

#### 0.1 客户端 Token 续签

- `POST /v4/remote-invoke/client/token/renew`
- Header:
  - `Authorization: Bearer <client_auth_token>`
- Body:
  - `client_instance_id`
  - `signature`（使用长期私钥签名 `client_instance_id + old_token_hash + timestamp`）
  - `timestamp`
- 响应：
  - 新 `client_auth_token`
  - `expires_at`
- 安全约束：
  - 旧 token 在 `5 分钟` grace period 内仍有效

#### 1. 客户端心跳

- `POST /v4/remote-invoke/client/heartbeat`

#### 2. 发布配对码

- `POST /v4/remote-invoke/client/pair-code`

#### 2.1 关闭发现模式

- `DELETE /v4/remote-invoke/client/discovery-session/:discovery_session_id`

#### 3. 提交授权决策

- `POST /v4/remote-invoke/client/grants/:pairing_id/decision`

#### 4. 回传调用输出帧

- `POST /v4/remote-invoke/client/calls/:call_id/frame`

#### 5. 回传调用结束

- `POST /v4/remote-invoke/client/calls/:call_id/exit`

#### 6. 确认撤销授权

- `POST /v4/remote-invoke/client/grants/:grant_id/revoke-ack`

### 首版命令协议

- 调用端不是人类交互页面，目标是**程序化接入**。
- 首版通过新增 `bifrost remote` 指令接入远程调用能力。
- 命令模型采用“受控查询命令白名单”，而不是透传 shell 字符串。
- 首版协议支持的完整命令列表：
  - `bifrost remote status`
  - `bifrost remote search <query>`
  - `bifrost remote traffic list [--limit N] [--cursor C] [--method GET] ...`
  - `bifrost remote traffic get <id> [--request-body] [--response-body]`
  - `bifrost remote traffic search <query>`
- 请求体中不上传原始 shell，而是上传结构化命令：

```json
{
  "command": "traffic.list",
  "args": {
    "limit": 50,
    "cursor": 12300,
    "method": "GET",
    "host": "api.example.com"
  }
}
```

```json
{
  "command": "traffic.get",
  "args": {
    "id": "57544",
    "request_body": true,
    "response_body": true
  }
}
```

- 客户端收到后只映射到本地允许的查询动作：
  - 调 `bifrost-cli` 只读命令
  - 或调用本地 admin/query 接口
- 白名单外命令直接拒绝，返回 `unsupported_command`。

### 加密信封格式

```json
{
  "version": 1,
  "call_id": "call_xxx",
  "seq": 12,
  "direction": "caller_to_client",
  "nonce": "base64",
  "ciphertext": "base64",
  "tag": "base64",
  "aad": {
    "token_hash": "sha256(token)",
    "frame_type": "stdin"
  }
}
```

### SSE 事件建议

- `event: status`
- `event: frame`
- `event: exit`
- `event: error`
- `event: heartbeat`

其中 `frame` 的 `data` 为加密信封；SSE 层只提供顺序和恢复能力，不负责解密。

### 客户端 SSE 事件建议

- `event: client_hello_ack`
- `event: pairing_request`
- `event: grant_created`
- `event: call_open`
- `event: call_frame`
- `event: call_cancel`
- `event: grant_revoked`
- `event: ping`

### `remoteInvoke.thrift` 草案

- 首版建议继续沿用 `bifrost-server-v4/app` 现有 `controller/service/idl` 分层方式。
- 由于客户端下行是 SSE，Thrift 侧主要定义：
  - 调用方 HTTP API
  - 客户端 HTTP 上行 API
  - 列表/详情查询 API
- 客户端 SSE 下行流本身可走普通 HTTP 路由，不强制经 Thrift 生成。

#### 建议结构

```thrift
namespace js bifrost.server.v4

struct CallerInfo {
  1: required string fingerprint,
  2: optional string display_name,
  3: optional string user_agent,
  4: optional string source_ip,
  5: optional string platform,
}

struct RemoteCommand {
  1: required string command,
  2: optional string args_json,
}

struct CommandSummary {
  1: required string command_preview,
  2: optional string masked_args_json,
  3: optional string payload_digest,
  4: optional i64 payload_size,
}

struct StartPairingReq {
  1: required string pair_code,
  2: required CallerInfo caller_info,
}

// 注意：client_instance_id 由 Relay 从 pair_code 自动解析，不再由 Caller 传入
// caller_pubkey、command_summary、command 在 connect 阶段不再需要（命令在 calls/open 时传入）

struct StartPairingData {
  1: required string pairing_id,
  2: required string status,
  3: required string approval_sse_url,
}

struct StartPairingRes {
  1: required byte code,
  2: required string message,
  3: required StartPairingData data,
}

struct ClientHeartbeatReq {
  1: required string client_instance_id,
  2: required string stream_id,
  3: optional list<string> active_call_ids,
}

struct PublishPairCodeReq {
  1: required string client_instance_id,
  2: required string pair_code,
  3: required i64 expires_at,
  4: optional string discovery_session_id,
}

struct GrantDecisionReq {
  1: required string pairing_id,
  2: required string client_instance_id,
  3: required string decision,
  4: optional string grant_mode,
  // 端到端加密已实现，`client_ephemeral_pub` 在 grant decision 时由 Client 携带，配合 Caller 的 `caller_ephemeral_pub` 派生 per-call 会话密钥
}

struct ClientCallFrameReq {
  1: required string call_id,
  2: required string client_instance_id,
  3: required string envelope_json,
}

struct ClientCallExitReq {
  1: required string call_id,
  2: required string client_instance_id,
  3: required i32 exit_code,
  4: optional i64 duration_ms,
  5: optional string stdout_digest,
  6: optional string stderr_digest,
  7: optional i64 bytes_in,
  8: optional i64 bytes_out,
}

struct RemoteInvokeRecord {
  1: required string id,
  2: required string client_instance_id,
  3: required string caller_fingerprint,
  4: required string command_preview,
  5: required string status,
  6: required string grant_mode,
  7: required i64 started_at,
  8: optional i64 ended_at,
  9: optional i32 exit_code,
}

// RemoteInvokeClient 结构已删除 — GET /clients 端点已完全移除，
// Relay 不再对外暴露客户端列表，客户端发现仅通过 pair_code 机制

struct ReusableGrant {
  1: required string id,
  2: required string client_instance_id,
  3: required string caller_fingerprint,
  4: required string grant_mode,
  5: required string status,
  6: optional i64 expires_at,
  7: optional i64 first_authorized_at,
  8: optional i64 last_used_at,
}
```

#### 服务接口建议

- `StartPairing`
- `GetPairingStatus`
- `OpenCall`
- `PostCallInput`
- `CancelCall`
- `GetReusableGrant`
- `DeleteGrant`
- `ClientHeartbeat`
- `PublishPairCode`
- `CloseDiscoverySession`
- `SubmitGrantDecision`
- `PostClientCallFrame`
- `PostClientCallExit`
- `ListRemoteInvokeCalls`
- `GetRemoteInvokeCall`
- `ListRemoteInvokeEvents`

### 标识与绑定字段定义

#### `client_instance_id`

- 语义：一个“正在运行中的 Bifrost 客户端实例”标识。
- 生成建议：
  - 首次启动生成随机 `128 bit` 实例 ID
  - 写入本地 data dir
  - 同一安装实例重启后继续复用
- 用途：
  - 标识授权和调用最终落到哪个客户端
  - 关联客户端 SSE 流、心跳和调用记录
  - 在多客户端场景下，作为 WebUI 管理和 CLI 选中目标客户端的主键
- 不建议每次进程重启都重新生成，否则永久授权与历史审计会失去稳定锚点。

#### `stream_id`

- 语义：某次客户端 SSE 连接的瞬时标识。
- 生成建议：
  - 每次建立 `/client/stream` 时生成新的随机 ID
- 用途：
  - 区分“同一客户端实例”的不同在线连接
  - Relay 只认最新活跃 `stream_id`
  - 老连接事件不再继续投递

#### `caller_fingerprint`

- 语义：调用方程序实例/设备的稳定指纹。
- 用途：
  - 绑定长期授权（grant）
  - 支撑风控和审计
  - 作为"授权复用命中"的关键维度
- **当前实现（2026-07-02）**：
  - Relay 以 Caller PoP Ed25519 公钥的 SPKI DER `sha256` hex 作为权威 `caller_fingerprint`，不信任请求体中自报的 fingerprint。
  - CLI 使用同一 PoP 公钥派生 `caller_fingerprint`，并用该值签名 `caller_ephemeral_pub`，保证 target 侧重建的签名 payload 与 caller 侧一致。
  - 旧的 `{BIFROST_DATA_DIR}/remote-caller-identity.json` 随机 fingerprint 仅作为本地兼容 fallback；安全决策以 relay-derived PoP fingerprint 为准。
- 要求：
  - 稳定可迁移，随 PoP 私钥持久化
  - 用户可手动删除 PoP 私钥触发身份轮换
  - 不直接暴露宿主机敏感信息

#### `call_id`

- 语义：一次实际远程查询调用的唯一标识。
- 生成方：Relay。
- 用途：
  - 关联输入、输出、SSE 事件、审计记录和清理逻辑。

#### `pairing_id`

- 语义：一次“一次性授权码验证成功后进入待授权”的请求标识。
- 生命周期短于 `call_id`，主要用于授权前阶段。

### 幂等、重连与恢复

#### 客户端 SSE 重连

- 客户端 SSE 断开后：
  - 立即进入指数退避重连
  - 带上新的 `stream_id`
  - 首次成功后立刻发 `ClientHeartbeat`
  - 同步上报 `active_call_ids`
- Relay 恢复时：
  - 用 `client_instance_id` 找回未完成调用
  - 将未完成的 `call_open` / `call_cancel` / 待投递 `call_frame` 重新下发

#### 调用方 SSE 重连

- 调用方订阅 `GET /calls/:call_id/events` 时允许携带：
  - `Last-Event-ID`
  - 或 `cursor`
- Relay 需在 Redis / DB 中短期缓存事件游标，确保断线可续。

#### HTTP 幂等

- 以下上行接口建议支持 `Idempotency-Key`：
  - `StartPairing`
  - `PostCallInput`
  - `SubmitGrantDecision`
  - `PostClientCallFrame`
  - `PostClientCallExit`
- 幂等窗口建议：
  - 调用创建：`5` 分钟
  - 调用帧：按 `call_id + seq + direction` 去重
  - 调用结束：按 `call_id` 仅允许首个终态写入

#### 帧顺序与去重

- 每个加密帧必须带：
  - `seq`
  - `direction`
  - `sent_at`
- Relay 只做：
  - 去重
  - 顺序缓存
  - 超时淘汰
- 不参与解密或改写业务内容。

### `bifrost remote` 执行时序

#### 调用端时序

1. 用户或程序执行：
   - `bifrost remote traffic get 57544`
2. CLI 加载本地连接文件 `{BIFROST_DATA_DIR}/remote-connections.json`。
3. CLI 通过 `resolve_local_connection()` 从本地文件中解析目标客户端：
   - 用户显式传 `--client-id <前缀>` → 在本地连接文件中按前缀匹配
   - 未传 `--client-id` → 如果仅有一个连接则直接使用，多个则交互选择
   - 0 个匹配 → 报错 "no saved connection, please run `bifrost remote connect <pair-code>` first"
4. CLI 从匹配的连接记录中获取 `client_instance_id`、`grant_id`、`caller_fingerprint`。
5. CLI 调用 `GET /grants/reusable?client_instance_id=X&caller_fingerprint=Y` 验证 grant 有效性。
   - 有效 → 继续
   - 无效 → 报错 "authorization expired, please run `bifrost remote connect <pair-code>` again"
6. CLI 组装结构化命令与摘要。
7. CLI 调用 `POST /calls/open {grant_id, client_instance_id, caller_fingerprint, command, command_summary}`。
8. Relay 校验 grant 绑定关系后签发 per-call `relay_token`，返回 `call_id` + `relay_token`。
9. CLI 订阅 `GET /calls/:call_id/events`（通过 `Authorization: Bearer <relay_token>` header）。
10. 收到 `frame/exit/error` 后流式输出到终端并退出。

#### 客户端时序

1. 本地 Bifrost 后台启动 `remote invoke worker`。
2. worker 建立 `client/stream` SSE。
3. worker 周期发送 `ClientHeartbeat`。
4. 用户在 Settings 里开启发现模式后，worker 调用 `PublishPairCode`。
5. 收到 `pairing_request` 后：
   - 写入本地 pending store
   - 推送通知
   - 弹出授权框
6. 用户批准后，worker 调用 `SubmitGrantDecision`。
7. 收到 `call_open` / `call_frame` 后：
   - 解密
   - 映射为本地只读查询
   - 执行
   - 按块调用 `PostClientCallFrame`
   - 完成后调用 `PostClientCallExit`

#### 本地执行器建议

- 不建议真的 fork 一个不受控 shell。
- 建议做一个受控执行器：
  - `RemoteQueryExecutor::execute(RemoteQueryCommand)`
- 内部映射两种来源：
  - 直接调用本地 admin/query handler
  - 或受控调用 `bifrost-cli` 只读子命令
- **参数安全约束**：
  - **禁止字符串拼接调用 shell**（禁止 `sh -c`），必须使用结构化参数传递（如 `Command::new().arg()`）。
  - 所有 `args` 字段必须经过**类型校验和白名单过滤**：
    - `id` 类参数：纯数字，最大长度 `20` 字符
    - `query` 类参数：支持 Unicode（含中文），禁止 ASCII 控制字符，最大长度 `500` 字符
    - 其他参数类型在白名单定义时明确允许的字符集和长度上限
  - 参数 sanitization 作为 `RemoteQueryExecutor` 的**入口强制步骤**，在命令分发前执行，校验失败直接返回 `invalid_args` 错误。
- 输出统一包装为：
  - `stdout`
  - `stderr`
  - `exit_code`
  - `summary`
  - `digests`

### 本地数据结构建议

#### 客户端内存态

- `active_stream`
- `active_discovery_session`
- `pending_pairings`
- `active_grants`
- `active_calls`

#### 本地持久态

- `remote_invoke_client.json`
  - `client_instance_id`
  - `device_id`
  - `device_keypair_ref`
- 本地不保存授权权威副本；可选保存轻量缓存：
  - `last_client_instance_id`
  - `last_grant_lookup_at`
  - `last_successful_grant_id`
- 可复用授权的权威记录存放在 Relay 服务端存储中（Redis + DB）。

#### Caller 本地连接文件

- **路径**：`{BIFROST_DATA_DIR}/remote-connections.json`（默认 `~/.bifrost/remote-connections.json`）
- Caller 端不再需要任何身份 token，连接信息完全基于 grant 凭证。
- **结构**：
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
- 不含 `caller_token` 字段。`grant_id` + `caller_fingerprint` 就是操作凭证。
- `resolve_local_connection()` 从此文件中按前缀匹配 `client_instance_id`，替代原来的 `resolve_client_id()`（不再调用 `GET /clients`）。

### Settings 内 Tab 细化

#### 状态区

- Relay 连接状态
- 最近心跳时间
- 当前 `stream_id`
- 当前发现模式状态
- 当前一次性授权码和剩余时间

#### ~~Clients 区~~ — 已简化

> 原设计中"在线客户端列表"部分已不适用，因为 `GET /clients` 端点已完全删除。
> 当前 Settings 内的 Remote Invoke Tab 聚焦于**本机客户端自身的状态管理**：
> - 发现模式状态和一次性授权码展示
> - 当前 Relay 连接状态
> - 进入/退出发现模式按钮

#### Pending Requests 区

- 待授权列表
- 来源设备指纹简写
- 命令摘要
- 批准/拒绝按钮

#### Active Grants 区

- 按调用方指纹分行展示
- 指纹
- 调用方显示名
- 授权模式
- 首次授权时间
- 最近使用时间
- 到期时间
- 活跃 call 数（该调用方当前正在进行的调用数）
- 移除授权按钮（Client 侧通过 Relay Client 路由管理）
- 刷新列表按钮

> **改造说明（2026-06-16）**：Caller 侧 `PATCH /v4/remote-invoke/grants/:id` 仍未提供。Client 侧管理端已新增 `PATCH /v4/remote-invoke/client/grants/:grant_id`（`handleUpdateGrantByClient`），允许 Client 自己调整已批准 grant 的属性；Caller 不可发起此请求。

#### Active Grants 交互要求

- 管理端（Client WebUI）支持对已授权的 grant 执行以下操作：
  - 直接移除授权（通过 Relay Client 路由或本地管理接口）
- 当管理端移除授权后：
  - grant 状态置为 `removed` 或 `revoked`
  - 后续 `bifrost remote` 不能再直接复用
  - 必须重新走配对授权流程

> **改造说明（2026-06-16）**：Caller 仍不能修改 grant。Client 管理端可通过 `PATCH /v4/remote-invoke/client/grants/:grant_id` 调整自己批准过的 grant；Client `DELETE /v4/remote-invoke/client/grants/:grant_id` 也可由 Client 端独立撤销。Caller 端仍只能通过 `DELETE /v4/remote-invoke/grants/:grant_id?caller_fingerprint=...` 在归属校验下撤销自己的 grant。

#### History 区

- 列表默认按时间倒序
- 支持按：
  - 客户端
  - 指纹
  - 命令类型
  - 状态
  - 时间范围
  - 授权模式
  过滤

### 服务端事件存储建议

#### `remote_invoke_event.event_type`

- `pairing_created`
- `discovery_started`
- `discovery_closed`
- `pairing_rejected`
- `pairing_approved`
- `grant_created`
- `grant_updated`
- `grant_revoked`
- `grant_removed`
- `call_opened`
- `call_frame_in`
- `call_frame_out`
- `call_completed`
- `call_failed`
- `call_cancelled`
- `client_disconnected`
- `client_reconnected`

#### `event_summary_json`

- 建议只保存摘要字段：
  - `seq`
  - `direction`
  - `frame_type`
  - `ciphertext_size`
  - `digest`
  - `reason`
- 明确不保存完整密文大包，避免 DB 膨胀。

## `bifrost-server-v4/app` 落地方案

### 迁移前提

- 只有当 `packages/bifrost-sync-server` 版本已经完成以下验证后，才进入 `bifrost-server-v4` 迁移：
  - 本地端到端主链路通过
  - 本地 human_tests 通过
  - 本地 E2E 通过
  - 接口字段与状态机基本稳定

### 云端迁移目标

- `bifrost-server-v4` 不是协议探索场，而是部署承载场。
- 迁移时应尽量复用已验证过的：
  - HTTP API 路径
  - SSE 事件名
  - 状态机
  - 数据表字段
  - 指纹、token、幂等语义

### 与 TCE 部署关系

- `bifrost-server-v4` 已有 TCE 部署清单，见现有 manifest。
- 因此远程调用 relay 的最终云端形态应落在 `bifrost-server-v4`，方便后续：
  - 发布
  - 组内环境部署
  - 与现有账号体系、SSO、中台服务联调

## `packages/bifrost-sync-server` 落地方案

### 为什么先落这里

- 它是一个零框架、原生 `http`、DAO 分层的轻量服务，协议试验成本更低。
- 现有能力已经具备：
  - 配置文件驱动
  - SQLite / MySQL 双存储
  - 基础鉴权
  - 清晰的 `routes/` + `dao/` 分层
- 对本地联调尤其友好：
  - 启动快
  - 依赖少
  - 易于直接增加测试脚本

### 建议新增目录

- `src/routes/remote-invoke.ts`
- `src/remote-invoke/`
  - `service.ts`
  - `crypto.ts`
  - `sse.ts`
  - `types.ts`
  - `cleanup.ts`
- `src/dao/remote-invoke.ts`
- `src/__tests__/remote-invoke.test.ts`
- `test/e2e-remote-invoke.sh`
- `sql/init-sqlite.sql` / `sql/init-mysql.sql` 增加远程调用表

### 建议模块映射

#### HTTP 路由层

- 参考 `src/routes/env.ts`、`src/routes/sso.ts` 风格，直接在原生 `http` 路由分发中挂载：
  - 调用方 API
  - 客户端上行 API
  - 客户端 SSE 下行流
  - 历史查询 API

#### 存储层

- 参考现有 DAO 模式，在 `src/dao/` 增加：
  - `IRemoteInvokeDao`
  - SQLite 实现
  - MySQL 实现
- 首次开发优先把 SQLite 跑通，再补 MySQL。
- DAO 需要显式支持：
  - `findReusableGrant(clientInstanceId, callerFingerprint)`
  - `getGrant(grantId)`
  - `deleteGrant(grantId)`
  - `touchGrantLastUsed(grantId, ts)`

> **改造说明**：原设计中的 `listGrants(userId, query)` 和 `updateGrant(grantId, patch)` 已移除。Caller 无需列出所有 grant（仅通过 `/grants/reusable` 查询具体授权），Grant 属性在 Client 审批时确定后不可修改。

#### 配置层

- 在 `src/types.ts` / `config.example.yaml` 增加：
  - `remote_invoke.enabled`（默认 `false`）
  - `remote_invoke.sse_keepalive_ms`（默认 `30000`，即 30 秒）
  - `remote_invoke.pair_code_ttl_secs`（默认 `120`，即 2 分钟）
  - `remote_invoke.max_active_calls_per_client`（默认 `5`）
  - `remote_invoke.max_active_calls_per_caller_per_client`（默认 `3`）
  - `remote_invoke.max_grants_per_client`（默认 `20`）
  - `remote_invoke.call_execution_timeout_secs`（默认 `60`）
  - `remote_invoke.retention_days`（默认 `90`）
  - `remote_invoke.max_records`（默认 `10000`）
  - `remote_invoke.max_sse_connections_per_client`（默认 `2`）
  - `remote_invoke.max_sse_connections_per_ip`（默认 `10`）
  - `remote_invoke.pair_rate_limit_per_ip`（默认 `5` 次/5 分钟）
  - `remote_invoke.pair_rate_limit_global_per_client`（默认 `10` 次/5 分钟）

### 本地 relay 版本的简化策略

- 本地版允许做以下简化，以加速验证：
  - 首版只支持 SQLite
  - 首版先不接 OAuth2，只用密码模式或测试 token
  - 首版不做多租户隔离（Caller 无用户概念，以 `caller_fingerprint` 维度隔离）
  - 首版先实现查询命令白名单，不引入额外命令扩展机制
- 但以下协议语义不能简化掉：
  - `client_instance_id`
  - `stream_id`
  - `caller_fingerprint`
  - 授权复用记录（存储在 Relay 服务端 DB 中）
  - 配对 / 授权 / 审计 全链路

### 本地验证完成的判定标准

- `packages/bifrost-sync-server` 中转能力完成以下事项后，才能视为可迁移：
  - 单机本地完整链路跑通
  - `bifrost remote` 成功调用查询命令
  - 二次执行命令可直接复用已有授权（grant），无需重新配对
  - 管理端可修改有效期并成功影响后续新调用
  - 管理端可移除授权并强制后续重新授权
  - 调用记录保留与清理策略正确
  - 断线重连与幂等主路径通过
  - 自动化测试与 human_tests 通过

### 文件级实施清单

#### 1. `src/types.ts`

- 新增远程调用配置与接口类型：
  - `RemoteInvokeConfig`
  - `ReusableGrant`
  - `RemoteInvokeCall`
  - `RemoteInvokeEvent`
  - `CallerInfo`
  - `RemoteCommand`
  - `GrantMode`
  - `GrantStatus`
  - `CallStatus`
- 在 `SyncServerConfig` 中增加：
  - `remote_invoke?: RemoteInvokeConfig`

#### 2. `src/dao/types.ts`

- 扩展 `IStorage`：
  - `remoteInvoke: IRemoteInvokeDao`
- 新增 `IRemoteInvokeDao`，建议方法：
  - `createPairing(...)`
  - `getPairing(pairingId)`
  - `consumePairCode(pairCode)`
  - `createGrant(...)`
  - `findReusableGrant(clientInstanceId, callerFingerprint)`
  - `getGrant(grantId)`
  - `deleteGrant(grantId)`
  - `touchGrantLastUsed(grantId, ts)`
  - `createCall(...)`
  - `getCall(callId)`
  - `updateCall(callId, patch)`
  - `appendEvent(...)`
  - `listCalls(query)`
  - `listCallEvents(callId, query)`
  - `cleanupExpiredData(now, retentionDays, maxRecords)`

#### 3. `src/dao/sqlite.ts` 与 `src/dao/mysql.ts`

- 补 `remoteInvoke` DAO 实现。
- 原则：
  - 先把 SQLite 版做完整
  - MySQL 跟着补齐同名语义
- 需要特别注意：
  - `grant` 查询要走 `client_instance_id + caller_fingerprint + status`
  - `call event` 写入频率高，要避免大字段与低选择性索引过多

#### 4. `src/dao/index.ts`

- `createStorage()` 中注册 `remoteInvoke` DAO。
- 确保返回的 `IStorage` 带上 `remoteInvoke` 成员。

#### 5. `src/http.ts`

- 新增 SSE 辅助函数：
  - `openSse(res)`
  - `writeSseEvent(res, event, data, id?)`
  - `writeSseComment(res, comment)`
  - `closeSse(res)`
- 新增通用头处理：
  - `Idempotency-Key`
  - `Last-Event-ID`
- 保持原生 `http` 风格，不引入额外框架。

#### 6. `src/remote-invoke/types.ts`

- 放远程调用内部领域类型，避免把 `src/types.ts` 撑得过大。
- 例如：
  - `PairingSessionRecord`
  - `GrantRecord`
  - `CallRecord`
  - `EventCursor`
  - `ClientStreamState`
  - `EncryptedEnvelope`

#### 7. `src/remote-invoke/crypto.ts`

- 放端到端加密与摘要工具：
  - `deriveSessionKeys(...)`
  - `hashPayload(...)`
  - `maskSensitiveArgs(...)`
  - `verifyCallerFingerprintBinding(...)`
- 这里只做 relay 所需的最小校验与摘要逻辑，不解密业务明文。

#### 8. `src/remote-invoke/sse.ts`

- 管理客户端 SSE 下行连接：
  - 记录当前活跃 `client_instance_id -> stream`
  - 支持覆盖旧 `stream_id`
  - 支持队列化下行事件
  - 支持 keepalive / ping
- 提供能力：
  - `registerClientStream(...)`
  - `unregisterClientStream(...)`
  - `pushToClient(...)`
  - `broadcastClientDisconnect(...)`

#### 9. `src/remote-invoke/service.ts`

- 放主要业务编排逻辑，是本地 relay 的核心。
- 建议分函数：
  - `startPairing()`
  - `approvePairing()`
  - `rejectPairing()`
  - `resolveReusableGrant()`
  - `openCall()`
  - `postCallerInput()`
  - `postClientFrame()`
  - `postClientExit()`
  - `removeGrant()`
  - `listHistory()`
- 原则：
  - 路由层只做解析
  - service 层做状态机与幂等

#### 10. `src/remote-invoke/cleanup.ts`

- 专门负责：
  - 配对码过期清理
  - 过期授权清理/状态刷新
  - 调用记录 `90` 天 / `10k` 清理
  - SSE 断开后残留状态清理

#### 11. `src/routes/remote-invoke.ts`

- 新增原生 HTTP 路由处理器。
- 统一处理四类接口：
  - 调用方请求
  - 客户端 SSE 下行
  - 客户端 HTTP 上行
  - 管理端列表/详情/授权管理

#### 12. `src/index.ts`

- 在主 server 路由分发中接入：
  - `if (await handleRemoteInvoke(ctx, storage)) return;`
- 位置建议：
  - 放在已认证业务路由附近
  - 保持 `/v4/...` 前缀一致

#### 13. `sql/init-sqlite.sql` 与 `sql/init-mysql.sql`

- 新增：
  - `bifrost_remote_invoke_pairings`
  - `bifrost_remote_invoke_grants`
  - `bifrost_remote_invoke_calls`
  - `bifrost_remote_invoke_events`
- SQLite 与 MySQL 字段语义必须保持一致，避免后续迁移歧义。

#### 14. `src/__tests__/remote-invoke.test.ts`

- 覆盖 service/dao 主逻辑：
  - 配对码消费
  - 可复用授权命中
  - 修改有效期
  - 删除授权
  - 调用终态写入
  - 清理逻辑

#### 15. `test/e2e-remote-invoke.sh`

- 做本地 relay 端到端脚本，至少覆盖：
  - 启动本地 relay
  - 注册/登录获取 token
  - 模拟客户端建立 SSE
  - 发布配对码
  - 调用方发起 pairing
  - 客户端批准
  - 调用方收到结果
  - 二次调用复用授权
  - 管理端修改有效期
  - 管理端删除授权
  - Caller 无需 token 即可完成配对和命令执行（验证 Caller 路由无鉴权）

### 路由映射表

#### 调用方接口

- `GET /v4/remote-invoke/grants/reusable`
  - 认证：**无需身份认证**（Caller 不需要 token，通过 `caller_fingerprint` 参数校验 grant 绑定）
  - 作用：查询可复用授权
- ~~`GET /v4/remote-invoke/grants`~~ — **已删除**（Caller 无需列出所有 grant，仅通过 `/grants/reusable` 查询具体授权）
- ~~`PATCH /v4/remote-invoke/grants/:grant_id`~~ — **已删除**（Grant 属性由 Client 审批时决定，Caller 不应修改）
- `DELETE /v4/remote-invoke/grants/:grant_id`
  - 认证：**无需身份认证**（通过 `caller_fingerprint` query 参数校验归属）
  - 作用：移除授权
- `POST /v4/remote-invoke/pairings/start`
  - 认证：**无需身份认证**（`pair_code` 本身就是发现门控）
  - 作用：发起配对请求（只需 `pair_code` + `caller_info`，不需要 `client_instance_id`）
- `GET /v4/remote-invoke/pairings/:pairing_id/watch`
  - 认证：**无需身份认证**（`pairing_id` 是一次性 UUID）
  - 作用：SSE 观察授权状态
- `POST /v4/remote-invoke/calls/open`
  - 认证：**无需身份认证**（通过 `grant_id` + `caller_fingerprint` 校验绑定关系）
  - 作用：创建调用，Relay 签发 per-call `relay_token`
- `POST /v4/remote-invoke/calls/:call_id/input`
  - 认证：`Authorization: Bearer <relay_token>`
  - 作用：发送加密输入帧
- `GET /v4/remote-invoke/calls/:call_id/events`
  - 认证：`Authorization: Bearer <relay_token>`
  - 作用：SSE 接收调用结果
- `POST /v4/remote-invoke/calls/:call_id/cancel`
  - 认证：`Authorization: Bearer <relay_token>`
  - 作用：取消当前调用

#### 客户端接口

- `GET /v4/remote-invoke/client/stream`
  - 认证：客户端 token
  - 作用：建立 SSE 长连接
- `POST /v4/remote-invoke/client/heartbeat`
  - 认证：客户端 token
  - 作用：保活 + 上报活跃调用
- `POST /v4/remote-invoke/client/pair-code`
  - 认证：客户端 token
- 作用：发布本地一次性授权码
- `POST /v4/remote-invoke/client/grants/:pairing_id/decision`
  - 认证：客户端 token
  - 作用：批准/拒绝 pairing
- `POST /v4/remote-invoke/client/calls/:call_id/frame`
  - 认证：客户端 token
  - 作用：回传输出帧
- `POST /v4/remote-invoke/client/calls/:call_id/exit`
  - 认证：客户端 token
  - 作用：回传调用终态
- `POST /v4/remote-invoke/client/grants/:grant_id/revoke-ack`
  - 认证：客户端 token
  - 作用：确认本地 grant 已失效

#### 管理端接口

- `GET /v4/remote-invoke/calls`
  - 认证：`client_auth_token`（Client 侧管理端认证）
  - 作用：历史列表
- `GET /v4/remote-invoke/calls/:call_id`
  - 认证：`client_auth_token`
  - 作用：调用详情
- `GET /v4/remote-invoke/calls/:call_id/events`
  - 认证：`client_auth_token`
  - 作用：管理端查看事件时间线

### SQL 落地建议

#### `bifrost_remote_invoke_pairings`

- 字段建议：
  - `id`
  - `client_instance_id`
  - `caller_fingerprint`
  - `pair_code`
  - `status`
  - `caller_display_name`
  - `caller_source_ip`
  - `expires_at`
  - `create_time`
  - `update_time`
- 索引建议：
  - `(pair_code, status)`
  - `(client_instance_id, status)`
  - `(expires_at)`

> **改造说明**：移除了 `user_id`、`caller_pubkey`、`client_ephemeral_pub`、`command_summary_json` 字段。`user_id` 不再需要（Caller 无身份概念）；`command_summary_json` 在 connect 阶段不再携带（命令在 calls/open 时传入）。端到端加密已实现：`caller_ephemeral_pub` / `client_ephemeral_pub` 改为在 grant 创建 / 调用建立时按 per-call 维度传递，配合 `grant_crypto_store` 存储，不再固化到 pairing 表。

#### `bifrost_remote_invoke_grants`

- 字段建议：
  - `id`
  - `client_instance_id`
  - `caller_fingerprint`
  - `grant_mode`
  - `grant_scope`
  - `status`
  - `first_authorized_at`
  - `expires_at`
  - `last_used_at`
  - `created_by`
  - `update_time`
- 索引建议：
  - `(client_instance_id, caller_fingerprint, status)`
  - `(status, expires_at)`
  - `(expires_at)`

#### `bifrost_remote_invoke_calls`

- 字段建议：
  - `id`
  - `grant_id`
  - `pairing_id`
  - `client_instance_id`
  - `caller_fingerprint`
  - `status`
  - `command_summary_json`
  - `payload_digest`
  - `stdout_digest`
  - `stderr_digest`
  - `exit_code`
  - `started_at`
  - `ended_at`
  - `duration_ms`
- 索引建议：
  - `(started_at DESC)`
  - `(client_instance_id, started_at DESC)`
  - `(grant_id)`
  - `(status, started_at DESC)`

#### `bifrost_remote_invoke_events`

- 字段建议：
  - `id`
  - `call_id`
  - `event_type`
  - `seq`
  - `direction`
  - `event_summary_json`
  - `create_time`
- 索引建议：
  - `(call_id, create_time)`
  - `(call_id, seq)`

### 推荐开发顺序

#### Step 1：打通存储与类型

- 先改：
  - `src/types.ts`
  - `src/dao/types.ts`
  - `sql/init-sqlite.sql`
  - `src/dao/sqlite.ts`
- 目标：
  - 先把 pairing/grant/call/event 的 SQLite 写读跑通

#### Step 2：打通服务层

- 再补：
  - `src/remote-invoke/types.ts`
  - `src/remote-invoke/service.ts`
  - `src/remote-invoke/cleanup.ts`
- 目标：
  - 状态机可单测
  - grant 复用、更新、删除逻辑可单测

#### Step 3：打通 HTTP 与 SSE

- 再补：
  - `src/http.ts`
  - `src/remote-invoke/sse.ts`
  - `src/routes/remote-invoke.ts`
  - `src/index.ts`
- 目标：
  - 客户端 SSE 能连上
  - 调用方 SSE 能看到事件

#### Step 4：补自动化测试

- 新增：
  - `src/__tests__/remote-invoke.test.ts`
  - `test/e2e-remote-invoke.sh`
- 目标：
  - 本地 relay 能覆盖授权复用与管理端更新

#### Step 5：补真实场景测试

- 新增：
  - `human_tests/remote-invoke.md`
  - `human_tests/readme.md` 索引
- 目标：
  - 本地完整闭环验证

### `bifrost remote` 与 relay 的接口映射

- `bifrost remote connect <pair_code>`
  - `POST /v4/remote-invoke/pairings/start`（只需 `pair_code` + `caller_info`）
  - `GET /v4/remote-invoke/pairings/:pairing_id/watch`（等待审批）
  - 审批通过后保存连接信息到 `{BIFROST_DATA_DIR}/remote-connections.json`
  - 注意：caller 链路不使用 `x-bifrost-token`，安全边界由 `pair_code`、`caller_fingerprint` 与 grant 绑定承担
- `bifrost remote traffic get 57544`
  - 从本地连接文件 `resolve_local_connection()` 解析目标
  - `GET /v4/remote-invoke/grants/reusable?client_instance_id=X&caller_fingerprint=Y` 验证 grant
  - 命中则 `POST /v4/remote-invoke/calls/open`
  - `GET /v4/remote-invoke/calls/:call_id/events`（SSE 接收结果）
  - 未命中则报错 "authorization expired, please run `bifrost remote connect <pair-code>` again"
- `bifrost remote status`
  - 同上流程：本地解析 → 验证 grant → 创建 call
- `bifrost remote disconnect [--client-id <前缀>] [--all]`
  - 从本地连接文件解析目标
  - `DELETE /v4/remote-invoke/grants/:grant_id?caller_fingerprint=Y`
  - 成功后从本地连接文件中移除记录
- 后续首版只读查询命令都遵循同一规则：
  - 先从本地连接文件解析，再查可复用授权，再创建 call 执行

### 新增模块

- `app/controller/remoteInvoke.ts`
- `app/service/remoteInvoke.ts`
- `app/model/remoteInvokeGrant.ts`
- `app/model/remoteInvokeCall.ts`
- `app/model/remoteInvokeEvent.ts`
- `app/idl/remoteInvoke.thrift`

### 是否复用现有 `share`

- 不建议直接复用 `share`。
- 原因：
  - `share` 更像“按名称保存 payload 的用户侧 session 存储”
  - 缺少在线 presence、授权策略、调用事件流、过期清理、多态状态机
- 可以借鉴其：
  - controller/service/idl 组织方式
  - `genID()` 生成主键模式

### Redis 用途

- `remote_invoke:client_presence:{client_instance_id}`
- `remote_invoke:pair_code:{pair_code}`
- `remote_invoke:pairing:{pairing_id}`
- `remote_invoke:call_route:{call_id}`
- `remote_invoke:grant_cache:{grant_id}`
- `remote_invoke:grant_lookup:{client_instance_id}:{caller_fingerprint}`
- `remote_invoke:rate_limit:{client_or_ip}`

### 数据库存储

#### 表 1：`remote_invoke_grant`

- `id`
- `client_instance_id`
- `caller_fingerprint`
- `grant_scope`
- `grant_mode`
- `first_authorized_at`
- `expires_at`
- `max_calls`
- `remaining_calls`
- `status`
- `created_by`
- `last_used_at`
- `updated_at`

说明：

- 此表同时承担"可复用授权记录"的职责。
- 对于 `packages/bifrost-sync-server`，它直接落在本地 SQLite 数据库中。
- 对于 `bifrost-server-v4`，迁移后同样应落在对应数据域（Redis + 持久化 DB），而不是仅存在临时缓存里。

#### 表 2：`remote_invoke_call`

- `id`
- `grant_id`
- `pairing_id`
- `client_instance_id`
- `caller_fingerprint`
- `source_ip`
- `caller_display_name`
- `command_summary_json`
- `payload_digest`
- `status`
- `started_at`
- `ended_at`
- `duration_ms`
- `exit_code`
- `stdout_digest`
- `stderr_digest`
- `bytes_in`
- `bytes_out`

#### 表 3：`remote_invoke_event`

- `id`
- `call_id`
- `event_type`
- `event_summary_json`
- `created_at`

### 清理策略

- 保留规则：
  - 超过 `90` 天删除
  - 总量超过 `10k` 时，按时间倒序保留最新 `10k`
- 与当前 `notification_db` 一致，采用：
  - 写入后低频触发清理
  - 时间过期优先
  - 数量上限兜底

## Bifrost 客户端落地方案

### 服务端能力

- 在 `bifrost-admin` 或新的 runtime 模块增加 `remote invoke manager`：
  - 管理 client SSE 连接
  - 管理活跃配对码
  - 管理 pending requests
  - 管理 grants 缓存
  - 管理正在执行的 call session

### 复用现有通知/推送路径

- 复用现有 `notification_db` 的模式，但**远程调用历史不直接塞进 notification 表**。
- 复用现有 `push.rs` 的消息分发框架，新增 Settings scope 或独立 push message：
  - `remote_invoke_pending`
  - `remote_invoke_grant_update`
  - `remote_invoke_history_update`
- 复用现有全局弹窗模式，参考：
  - `PendingAuthModal`
  - `AccessControlTab`

### 命令执行边界

- 首版只允许执行"Bifrost 自己定义的查询命令协议"，不要暴露任意 shell。
- 首版仅允许只读查询动作：
  - `status`
  - `search.get`
  - `traffic.list`
  - `traffic.get`
  - `traffic.search`
- 明确禁止：
  - 配置修改
  - 规则新增/编辑/删除
  - values/config/cert/system proxy 等管理操作
  - 任意本地文件访问
  - 脚本执行
  - `traffic.clear`（写操作，不在只读白名单内）
- 客户端执行层建议做成 `enum RemoteQueryCommand`，由结构化命令映射到本地只读能力。
- 即使未来扩展能力，也应继续走白名单扩展，不退回 shell 透传模型。

### 远程 traffic 子命令能力详述

远程调用支持的 traffic 子命令完整列表如下，均为只读查询操作。每个命令通过结构化 JSON 参数传递，由客户端执行器映射到本地 admin API。

#### `traffic.list` — 分页查询流量记录列表

- **用途**：按条件筛选并分页返回流量记录摘要列表。
- **本地 API**：`GET /_bifrost/api/traffic?<query_string>`
- **请求参数**（`args_json` 中的字段）：

| 参数 | 类型 | 必填 | 默认值 | 说明 | 安全约束 |
|------|------|------|--------|------|---------|
| `limit` | number | 否 | `50` | 每页记录数 | 强制上限 `100`，防止单次返回过多数据 |
| `cursor` | number | 否 | - | 分页游标（从上次响应的 `next_cursor`/`prev_cursor` 获取） | 纯数字 |
| `direction` | string | 否 | `"backward"` | 翻页方向：`backward`（最新→最旧）/ `forward`（最旧→最新） | 白名单校验 |
| `method` | string | 否 | - | 按 HTTP 方法筛选 | 白名单：`GET`/`POST`/`PUT`/`DELETE`/`PATCH`/`HEAD`/`OPTIONS`/`CONNECT`/`TRACE` |
| `status` | number | 否 | - | 按状态码精确筛选 | 范围 `100-599` |
| `status_min` | number | 否 | - | 状态码 >= 该值 | 范围 `100-599` |
| `status_max` | number | 否 | - | 状态码 <= 该值 | 范围 `100-599` |
| `protocol` | string | 否 | - | 按协议筛选 | 白名单：`http`/`https`/`ws`/`wss`/`h3` |
| `host` | string | 否 | - | 按域名包含匹配 | 长度 ≤ 200，字符集 `[a-zA-Z0-9._\-:*]` |
| `url` | string | 否 | - | 按 URL 包含匹配 | 长度 ≤ 500，字符集 `[a-zA-Z0-9._\-:/？&=%+~#@!$,;]` |
| `path` | string | 否 | - | 按路径包含匹配 | 长度 ≤ 500，字符集 `[a-zA-Z0-9._\-:/]` |
| `content_type` | string | 否 | - | 按 Content-Type 筛选 | 长度 ≤ 100 |
| `client_ip` | string | 否 | - | 按客户端 IP 筛选 | 长度 ≤ 45 |
| `client_app` | string | 否 | - | 按客户端应用筛选 | 长度 ≤ 200 |
| `has_rule_hit` | bool | 否 | - | 是否命中规则 | - |
| `is_websocket` | bool | 否 | - | 仅 WebSocket 请求 | - |
| `is_sse` | bool | 否 | - | 仅 SSE 请求 | - |
| `is_tunnel` | bool | 否 | - | 仅 CONNECT 隧道 | - |

- **响应内容**（JSON）：

```json
{
  "records": [
    {
      "id": "uuid",
      "seq": 12345,
      "m": "GET",
      "h": "api.example.com",
      "p": "/v1/users",
      "s": 200,
      "res_sz": 1024,
      "dur": 150,
      "proto": "https",
      "st": "2025-01-01T00:00:00Z"
    }
  ],
  "next_cursor": 12300,
  "prev_cursor": 12350,
  "has_more": true,
  "total": 500,
  "server_sequence": 12400
}
```

- **安全约束**：
  - `limit` 强制上限 `100`，客户端发送超过此值时自动截断。
  - 所有字符串筛选参数均需通过字符集白名单和长度上限校验，校验失败返回 `invalid_args`。

#### `traffic.get` — 获取单条流量记录详情

- **用途**：通过 ID 或 sequence 获取完整的流量记录详情，支持同时获取请求体和响应体。
- **本地 API**：
  - 详情：`GET /_bifrost/api/traffic/{id}`
  - 请求体：`GET /_bifrost/api/traffic/{id}/request-body`
  - 响应体：`GET /_bifrost/api/traffic/{id}/response-body`
- **请求参数**（`args_json` 中的字段）：

| 参数 | 类型 | 必填 | 默认值 | 说明 | 安全约束 |
|------|------|------|--------|------|---------|
| `id` | string | **是** | - | 流量记录 ID 或 sequence 编号 | 纯数字，长度 ≤ 20 |
| `request_body` | bool | 否 | `false` | 是否包含请求体 | - |
| `response_body` | bool | 否 | `false` | 是否包含响应体 | - |

- **响应内容**（JSON）：返回完整的流量记录详情，包含请求/响应头、timing 信息、TLS 信息等。当 `request_body=true` 时，响应中额外包含 `request_body` 字段；当 `response_body=true` 时，响应中额外包含 `response_body` 字段。
- **执行语义**：
  - 当 `id` 为纯数字时，客户端必须按本地 `bifrost traffic get <seq>` 的规则，先将 sequence 解析到真实 traffic id，再读取详情。
  - 解析过程应优先命中精确 sequence；若仅提供短后缀，则按最新记录优先匹配，并保持与本地 CLI 一致的候选顺序。
- **与本地 CLI 的差异**：远程版完整支持本地 `bifrost traffic get` 的所有能力（`--request-body`、`--response-body`、纯数字 sequence 解析），通过结构化参数传递。

#### `traffic.search` — 按关键词搜索流量记录

- **用途**：按关键词全文搜索流量记录（搜索范围覆盖 URL、headers、body）。
- **本地 API**：`POST /_bifrost/api/search/stream` body=`{"keyword": query, "limit": limit}`
- **别名**：`search.get`（功能完全等价）
- **请求参数**（`args_json` 中的字段）：

| 参数 | 类型 | 必填 | 默认值 | 说明 | 安全约束 |
|------|------|------|--------|------|---------|
| `query` | string | **是** | - | 搜索关键词 | 长度 ≤ 500，支持 Unicode（含中文），禁止 ASCII 控制字符 |
| `limit` | number | 否 | `50` | 最大返回结果数 | 强制上限 `100` |

- **响应内容**（stdout 流式文本）：
  - 远程调用必须复用本地 `bifrost search` 的 SSE 搜索链路，而不是阻塞式一次性查询。
  - 客户端逐步把搜索结果、进度和最终 summary 通过 relay frame 推送给 caller，caller 收到 frame 后立即写到终端。
  - 输出格式与本地 `bifrost search <keyword>` 的默认表格模式保持一致，至少包含结果行、搜索进度和最终 summary。
- **与本地 CLI 的差异**：远程版目前仍只支持 keyword + limit，不支持本地 `bifrost search` 的高级筛选参数（`--url`、`--headers`、`--body`、`--status`、`--method` 等范围限定）。后续可按需扩展。

#### 失败回传语义

- 当远程命令执行失败时，client 侧生成的错误文本必须通过 relay 原样回传给 caller，不能只返回 `exit_code = -1`。
- `call exit` 事件除 digest 外，还需要携带可选 `stderr` 字段，供 caller 在终端直接展示。
- 对流式命令（当前为 `traffic.search` / `search.get`），caller 需要边收 frame 边输出；对非流式命令，继续保持“完整收集后一次性输出”模式。

## 2026-04-20 回归修复

### 问题现象

1. `bifrost remote traffic get 566961` 在远端有数据时仍返回 `Remote command 'traffic.get' exited with code -1`
2. `bifrost remote search nextoncall` 在 client 侧存在匹配数据时返回 `exit code -1`
3. 失败时 caller 终端看不到真实错误原因，只能看到 `-1`

### 根因

1. `traffic.get` 远程执行器直接把纯数字入参当作真实 traffic id 请求 `/api/traffic/{id}`，没有复用本地 CLI 的 sequence 解析逻辑。
2. `search.get` / `traffic.search` 远程执行器仍走阻塞式 `POST /api/search`，并受 30s 请求超时限制；在大流量库上无法像本地 `bifrost search` 一样边搜边回传结果。
3. relay 的 `call exit` 事件只回传 digest，不回传实际 `stderr` 文本，导致 caller 无法展示真实错误。

### 修复方案

1. 在 remote executor 中补齐 sequence -> real id 解析，保证 `remote traffic get <seq>` 与本地 CLI 语义一致。
2. 将远程搜索切换到 `/api/search/stream`，在 client 侧把 SSE 事件转换为终端文本并通过多帧 relay 输出。
3. 扩展 `ClientCallExitRequest` / relay exit 事件，增加可选 `stderr` 字段并在 caller 侧展示。

### 验证计划

- 单元测试：
  - `remote_invoke::executor` 覆盖纯数字 sequence 解析与非法 id 拒绝
  - `remote CLI` 覆盖 exit 事件中的 `stderr` 展示与流式 frame 输出行为
- E2E：
  - 扩展 `e2e-tests/tests/test_remote_invoke_e2e.sh`
  - 回归 `remote traffic get <seq>` 能正确返回详情
  - 回归 `remote search <keyword>` 能返回命中结果，且运行期间提前产生输出
- Human tests：
  - 更新 `human_tests/remote-invoke.md`
  - 逐条执行 sequence 查询、流式搜索、错误透传三个回归场景

#### `traffic.clear` — 不支持

- `traffic.clear` 为写操作（删除流量记录），不在只读白名单内，远程调用明确拒绝。
- 发送 `traffic.clear` 命令会收到 `unsupported_command` 错误。

## WebUI 设计

### 1. Settings 新增 `Remote Invoke` Tab

- 基础开关：
  - 是否启用远程调用
  - 首版固定为“仅允许查询命令”
  - 默认授权策略
- 当前状态：
  - Relay 连接状态
  - 当前客户端实例 ID
  - 当前一次性授权码
  - 一次性授权码剩余时间
- 授权管理：
  - 活跃授权列表
  - 授权模式
  - 过期时间
  - 最近使用时间
  - 手动撤销

### 2. 全局授权弹窗

- 当有新的配对请求时，全局弹出通知。
- 展示字段：
  - 调用方设备指纹（`caller_fingerprint`，基于 username+hostname 的 hash 截短展示）
  - 调用方显示名
  - 来源 IP / 地域（如果可得）
  - User-Agent
  - 命令摘要
  - 请求时间
  - 新设备标记（若该 `caller_fingerprint` 在历史授权中无记录，醒目标注"⚠️ 新设备，请确认是否为本人操作"）
- 操作按钮：
  - 拒绝
  - 本次允许
  - 允许 `30m`
  - 允许 `1h`
  - 允许 `1d`
  - 永久允许

### 3. 新增 `Remote Invoke` 页面

- 历史记录与授权管理都放在 `Settings -> Remote Invoke` 下，不新增一级导航。
- Tab 内提供：
  - 调用记录列表
  - 事件时间线
  - 详情抽屉/详情区
  - 过滤器（状态/来源/客户端/时间范围）

### 4. 列表字段

- 时间
- 来源
- 客户端
- 授权方式
- 命令摘要
- 输入摘要
- 输出摘要
- 状态
- 耗时
- 退出码

### 5. 详情字段

- call 基本信息
- pairing / grant 关系
- 事件时间线
- 明文摘要
- 原始载荷 digest
- 加密帧统计

## 审计与摘要策略

### 必须保存

- 谁发起的
- 什么时候发起的
- 来源 IP / UA
- 作用到哪个客户端
- 用户选了哪种授权方式
- 调用了什么命令摘要
- 输入输出摘要
- 结果如何

### 不保存

- 原始命令明文
- 原始输出明文
- 会话密钥
- 任何可直接复原敏感内容的材料

### 摘要建议

- `command_preview`
  - 例如：`bifrost search 57544`
- `masked_args`
  - 对 token、cookie、password 自动掩码
- `payload_digest`
  - `sha256`
- `payload_size`
- `stdout_digest`
- `stderr_digest`
- `first_line_preview`
  - 最多 `120` 字符

### 审计日志完整性保障

- **链式哈希**：每条审计记录包含前一条记录的 `sha256` 哈希值，形成 append-only hash chain。篡改任何中间记录会导致后续链断裂。
- **客户端本地副本**：客户端在本地维护一份轻量级审计摘要（仅包含 `call_id`、`command_preview`、`timestamp`、`chain_hash`），用于事后比对 Relay 侧记录是否被篡改。
- **残余风险声明**：如果 Relay 被完全攻破且攻击者能重建完整 hash chain，则此机制可被绕过。完全防篡改需要引入外部可信时间戳服务（如 RFC 3161），暂列为未来增强项。

## 风险与防护

### 风险 1：一次性授权码爆破

- 防护：
  - 单码 TTL `2` 分钟，超时自动轮换为全新随机码，旧码立即作废
  - 失败限流
  - 单码单次消费
  - 可选设备指纹绑定

### 风险 2：授权疲劳

- 防护：
  - 命令摘要必须清晰
  - 高风险命令高亮
  - 默认使用“一次调用”
  - 永久授权需要二次确认

### 风险 3：Relay 偷看明文

- 防护：
  - 端到端加密
  - Relay 仅存摘要
  - token 只做绑定，不做明文保护

### 风险 4：grant 凭证被窃取

- 防护：
  - `grant_id` 与 `caller_fingerprint` 双因子绑定，窃取 `grant_id` 还需知道对应的 `caller_fingerprint`
  - `caller_fingerprint` 基于 username+hostname 生成，攻击者需知道目标机器信息
  - grant 有时效策略（Once/30m/1h/1d/Permanent），过期自动失效
  - 支持手动 revoke
  - SSE/HTTP 全程 HTTPS

### 风险 5：客户端掉线

- 防护：
  - Relay 维护 presence TTL
  - 掉线时 pending request 自动失败
  - call 中断写入事件并允许调用方感知
  - SSE 断开后客户端按退避策略自动重连，并重新发送心跳与未完成调用状态

### 风险 6：大流量传输导致内存或连接被占满

- 防护：
  - 分片帧传输
  - 每客户端 / 每调用限流
  - Relay 背压控制
  - 大分片短期游标缓存 + 可选临时文件落盘
  - 历史库仅保存摘要，不保存大正文

### 风险 7：SSE 连接数 DoS

- 防护：
  - 单个 `client_instance_id` 最多 `2` 条并发 SSE 连接（1 条活跃 + 1 条重连过渡）
  - 单个 IP 最多 `10` 条并发 SSE 连接
  - 超出上限的连接直接拒绝（`429 Too Many Requests`）

### 风险 8：私钥泄露

- 防护：
  - 客户端长期私钥和 ephemeral 私钥**禁止写入日志**（tracing 输出中禁止出现私钥材料）
  - 长期私钥存储建议优先使用 OS keychain / credential store（macOS Keychain、Windows Credential Manager、Linux Secret Service）
  - 如使用文件存储，权限必须设置为 `0600`（仅 owner 可读写）
  - ephemeral 私钥仅存在于内存中，call 结束后立即 zeroize 清除

### 风险 9：Discovery 模式客户端枚举

- 防护：
  - **`GET /v4/remote-invoke/clients` 端点已完全删除**，不保留任何访问入口
  - Relay 绝不主动暴露注册的客户端信息
  - 客户端发现仅通过 `pair_code` 机制（6 位一次性码，2 分钟 TTL）
  - 配对成功前，Caller 无法获知任何 `client_instance_id` 或设备信息
  - 只有 Client 用户审批通过后，Caller 才能获取 `client_instance_id`、`device_name`、`platform` 等信息

### 风险 10：时钟偏差导致过期判断异常

- 防护：
  - `expires_at` 校验允许 `±30 秒` 的时钟偏差容忍
  - 建议 Relay 签发的 token/grant 使用 Relay 服务器时间戳，客户端在收到时记录 `server_time - local_time` 偏移量用于本地校验

### 风险 11：caller_fingerprint 轮换导致授权丢失

- 防护：
  - `caller_fingerprint` 轮换时，调用方需要通过 Relay 发起 fingerprint 迁移请求
  - 迁移请求需要携带旧 fingerprint 对应的有效 `relay_token`，证明调用方确实拥有旧身份
  - Relay 将旧 fingerprint 绑定的所有 grant 迁移到新 fingerprint，并记录迁移审计日志
  - 如果旧 fingerprint 已无有效 token，则必须重新配对授权

## 分阶段实施建议

### Phase 1：最小闭环（已发布）

> **实现状态更新（2026-06-16）**：首版 Phase 1 已实质完成。`EncryptedEnvelope` v2 已落地为真正密文（X25519 + HKDF-SHA256 + ChaCha20-Poly1305 / AES-256-GCM），并已扩展支持 SSH 公钥免配对授权链路；命令白名单已从只读查询扩展到 `RemoteShellExec` / `RemoteShellInteractive` / `RemotePowerMgmt` / `RemoteImGateway`，叠加独立的 `FileAccessScope`。

- 客户端在线 SSE
- 发现模式与一次性授权码
- 6 位码配对（Relay 从 pair_code 自动解析 client_instance_id）
- 本地人工授权
- 一次调用
- `bifrost remote` 程序化调用入口（基于本地连接文件，无需 Caller token）
- 仅只读查询命令白名单
- 多客户端在线管理
- 大结果分片流式传输
- SSE 输出流
- 调用记录落库
- grant_id + caller_fingerprint 双因子操作凭证
- per-call relay_token 路由

### Phase 2：完整授权 + 安全增强

### Phase 2：完整授权 + 安全增强（部分已发布）

- ✅ `30m/1h/1d/永久` 授权策略
- ✅ grant 管理与撤销（Client 侧 PATCH/DELETE 路由 `/v4/remote-invoke/client/grants/:grant_id`）
- ✅ WebUI 历史页
- ✅ 失败重连 / 恢复
- ✅ client_auth_token 注册（Ed25519 challenge/response）
- ✅ 端到端加密：X25519 + HKDF-SHA256 + ChaCha20-Poly1305 / AES-256-GCM 加密帧
- ⏳ 设备指纹可信绑定与迁移（planned, not yet shipped as of 2026-06-16）
- ⏳ `client_auth_token` 续期端点（`POST /client/token/renew` 仍未实现，依赖重新注册替代）

### Phase 3：高级安全与运维

- 风险命令分类与策略控制
- 监控告警体系（见下方"监控与告警"章节）
- 审计日志完整性验证（hash chain）
- Relay 降级保护策略

## Relay 不可用降级策略

当 Relay 服务不可达或响应超时时，系统需有明确的降级行为，而非静默失败：

### 客户端侧

- SSE 连接断开后进入指数退避重连（初始 `1s`，上限 `60s`，抖动 ±20%）
- 重连期间客户端本地状态机保持"SSE 断连"状态，WebUI 展示明确的 Relay 离线提示
- 已获授权的 grant 在本地缓存中保留有效期，但不允许在 Relay 离线期间发起新 call（因为加密帧无法经 Relay 转发）
- 如果 Relay 持续不可达超过 `5` 分钟，客户端自动退出发现模式并清除内存中的 pair_code

### 调用方侧

- `bifrost remote` 在创建 call 或发送 SSE 订阅失败时，返回明确的 `RelayUnavailable` 错误码（不是泛化的网络超时）
- 调用方 CLI 输出 Relay 不可达的诊断信息（Relay 地址、最后成功连接时间、建议操作）
- 如果调用方持有有效 `relay_token`，允许在 Relay 恢复后自动恢复 SSE 订阅（前提是 token 未过期且 grant 仍有效）

### Relay 侧

- Relay 重启后必须从持久化存储恢复所有未过期的 grant 和 active call 元数据
- Relay 不主动清理因自身重启而断开的 SSE 连接对应的 call，而是等待客户端重连后恢复或等待 TTL 自然过期
- Relay 应提供 `GET /health` 端点，返回当前在线客户端数、活跃 call 数、Redis 连接状态，方便上游监控

## 多调用方并发支持

### 场景说明

同一个 Bifrost 客户端可能被多个远程调用方（不同的 `caller_fingerprint`）同时使用。例如：
- 同事 A 和同事 B 各自通过 `bifrost remote` 同时查询同一个 Bifrost 实例的流量
- 同一个人在不同设备上同时发起远程调用
- 自动化脚本和人工调用并发进行

方案必须在配对、授权、执行、资源隔离四个层面明确支持多调用方并发。

### 多 caller 配对排队

- 一次性授权码（pair_code）是**一次消费**的：第一个成功验证 pair_code 的调用方获得配对机会，后续调用方对同一 pair_code 的验证请求返回 `pair_code_already_consumed`。
- **配对是串行的**：同一客户端同一时刻只允许一个活跃配对流程（`pending_approval` 状态）。
- 当第一个调用方的配对被批准或拒绝后，用户可重新开启发现模式生成新 pair_code，供下一个调用方使用。
- 调用方在配对被占用时收到明确的错误码 `pair_slot_occupied`，应提示"目标设备当前有其他配对请求正在等待审批，请稍后重试"。

### 多 caller 并发授权（grant）

- 授权（grant）按 `caller_fingerprint` 维度独立管理。
- **同一客户端可同时存在多个来自不同调用方的活跃 grant**。每个 grant 绑定 `(client_instance_id, caller_fingerprint)` 二元组。
- grant 之间完全隔离：调用方 A 的 grant 撤销不影响调用方 B。
- 可复用授权查询 `findReusableGrant(clientInstanceId, callerFingerprint)` 天然按 caller 隔离。
- 配置约束：
  - `remote_invoke.max_grants_per_client`（默认 `20`）— 同一客户端允许的最大活跃 grant 数（跨所有 caller），超出后新配对被拒绝并返回 `grant_limit_exceeded`。

### 多 caller 并发调用（call）

- **同一客户端允许多个调用方同时发起 call**，各 call 独立执行、独立加密、独立回传结果。
- `max_active_calls_per_client`（默认 `5`）约束的是**同一客户端上所有调用方的并发 call 总数**，而非每个调用方独享。
- 新增 `remote_invoke.max_active_calls_per_caller_per_client`（默认 `3`）— 约束单个调用方对单个客户端的并发 call 上限，防止单个 caller 独占全部配额。
- 当并发 call 数达到上限时，新的 call 请求返回 `call_limit_exceeded`，调用方应排队重试。

### 客户端执行器并发

- `RemoteQueryExecutor` 必须支持并发执行多个只读查询命令。
- 由于首版仅支持只读查询命令（`status`、`traffic.list`、`traffic.get`、`traffic.search`、`search.get`），多命令并发不存在写冲突。
- 执行器内部使用独立的 task/future 处理每个 call，通过 `call_id` 隔离上下文。
- 资源保护：
  - 执行器并发度上限与 `max_active_calls_per_client` 保持一致。
  - 单个 call 超时 `60s`，超时后执行器强制终止并返回 `execution_timeout`。
  - 执行器队列满时返回 `executor_busy`，Relay 向调用方转发此错误。

### 客户端 SSE 下行复用

- 多个并发 call 的下行帧（`call_frame`、`call_open`、`call_cancel` 等）共享同一条客户端 SSE 连接。
- 每个下行事件通过 `call_id` 字段区分归属，客户端按 `call_id` 分发到对应的执行 task。
- 客户端心跳中上报 `active_call_ids` 列表，Relay 据此恢复断线重连后的未完成调用。

### 调用方之间的隔离性

- **数据隔离**：不同调用方的加密会话密钥完全独立（per-call ephemeral key），调用方 A 无法解密调用方 B 的帧。
- **授权隔离**：grant 按 `caller_fingerprint` 绑定，一个调用方的 `relay_token` 无法代表另一个调用方发起操作。
- **可见性隔离**：调用方只能通过自己的 `relay_token` 订阅自己发起的 call 事件流，无法窥探其他调用方的 call。
- **WebUI 可见性**：Bifrost 客户端的 WebUI（管理端视角）可以看到所有调用方的授权和调用记录，这是运维需要。

### WebUI 多授权请求排队展示

- 当多个调用方先后发起配对请求时，由于配对串行，同一时刻只有一个 `pending_approval`。
- WebUI 的 `Pending Requests` 区按请求到达顺序展示，用户逐个审批。
- 审批完一个后，如果发现模式仍开启且有新的配对请求到达，自动展示下一个。
- 已授权的多个调用方同时出现在 `Active Grants` 区，按 `caller_fingerprint` 分行展示，每行显示调用方设备指纹、授权模式、过期时间和最近使用时间。

## 并发配对冲突处理

### 场景

同一客户端当前有一个 pair_code 处于 `pending_approval`（调用方已提交配对，等待用户在 WebUI 审批），此时用户或系统触发新的发现模式请求。

### 处理策略

- **同一客户端同一时刻只允许一个活跃配对流程**
- 如果当前已有处于 `pending_approval` 的配对请求：
  - 新的发现模式请求会**先取消旧的 pending 配对**（向等待中的调用方推送 `pair_cancelled` 事件），然后生成新的 pair_code
  - 旧调用方收到 `pair_cancelled` 后应提示用户"配对已被目标设备取消"
- 如果当前 pair_code 尚未被任何调用方消费（纯等待中）：
  - 新的发现模式请求直接替换旧码，旧码立即失效
- Relay 侧使用 `client_instance_id` 做排他锁保证同一客户端不会出现两个并行的配对状态
- **多调用方配对冲突**：当调用方 A 的配对正处于 `pending_approval`，调用方 B 尝试用同一 pair_code 配对时，返回 `pair_code_already_consumed`；如果调用方 B 用新码配对，因为配对槽被占用，返回 `pair_slot_occupied`

## 监控与告警

### 关键指标（Metrics）

| 指标名 | 类型 | 说明 |
| --- | --- | --- |
| `remote_invoke.pair_attempts_total` | Counter | 配对尝试总数（按 `status=success/failed/expired` 分标签） |
| `remote_invoke.pair_failure_rate_5m` | Gauge | 5 分钟滑动窗口配对失败率 |
| `remote_invoke.active_grants` | Gauge | 当前活跃 grant 数 |
| `remote_invoke.active_calls` | Gauge | 当前活跃 call 数 |
| `remote_invoke.sse_connections` | Gauge | 当前 SSE 连接数（按 `role=caller/client` 分标签） |
| `remote_invoke.call_duration_seconds` | Histogram | 单次 call 持续时长 |
| `remote_invoke.relay_token_revocations_total` | Counter | token 撤销总数（按 `reason=user/expired/security` 分标签） |
| `remote_invoke.encryption_failures_total` | Counter | 加密/解密失败次数（可能指示密钥不同步） |

### 告警规则

| 告警名 | 条件 | 严重级别 | 说明 |
| --- | --- | --- | --- |
| `HighPairFailureRate` | `pair_failure_rate_5m > 0.5` | Warning | 配对失败率过高，可能遭受爆破攻击 |
| `BruteForceDetected` | 单 IP 5 分钟内 pair 失败 > `20` 次 | Critical | 触发 IP 级临时封禁（`15` 分钟） |
| `AbnormalCallerGeo` | 同一 `caller_fingerprint` 短时间内来自不同地理区域 | Warning | 可能指示凭据泄露 |
| `SSEConnectionStorm` | 单 `client_instance_id` SSE 重连 > `30` 次/分钟 | Warning | 客户端异常重连风暴 |
| `EncryptionFailureSpike` | `encryption_failures_total` 增长 > `10`/分钟 | Critical | 密钥同步可能已破坏，需人工介入 |

### 日志审计关注点

- 所有配对失败事件必须记录：`caller_ip`、`client_instance_id`、`failure_reason`、`timestamp`
- 所有 grant 创建/撤销/过期事件必须记录完整审计链
- 异常事件（如连续失败触发冷却、IP 封禁）需输出 `WARN` 级别日志并携带结构化字段

## 测试方案

### 单元测试

- `pair_code_generate_unique_and_expire`
- `pair_code_verify_rate_limit`
- `grant_once_consumed_after_call`
- `grant_timebox_expires_correctly`
- `relay_token_bound_to_fingerprint`
- `envelope_encrypt_decrypt_roundtrip`
- `call_cleanup_by_age_and_count`
- `masked_summary_hides_sensitive_args`

### E2E 测试

- 验证调用方通过一次性授权码发起请求后，本地弹出授权
- 验证拒绝后调用方收到 `rejected`
- 验证一次调用授权后，仅首个 call 成功
- 验证 `30m` 授权下多次调用可复用 grant
- 验证客户端断线时 SSE 收到中断事件
- 验证加密帧经 Relay 转发但服务端日志不出现明文
- 验证多调用方各自配对授权后可并发 call 同一客户端
- 验证多调用方之间 grant 隔离：撤销一个不影响另一个
- 验证并发 call 超过 `max_active_calls_per_client` 时返回 `call_limit_exceeded`
- 验证 `90` 天和 `10k` 上限清理策略

### 真实场景测试

- 在 `human_tests/remote-invoke.md` 编写并执行：
  - 发现模式开启/关闭与一次性授权码生成
  - 授权码一次性消费与过期
  - 全局弹窗授权
  - 本次授权
  - `30m/1h/1d/永久` 授权
  - 多客户端在线管理与客户端选择
  - 多调用方并发授权与独立调用（两个不同 caller 各自配对并同时查询同一 Bifrost 客户端）
  - 多调用方 grant 隔离（撤销调用方 A 的授权不影响调用方 B）
  - 并发 call 资源上限验证（超过 `max_active_calls_per_client` 时正确拒绝）
  - 大结果流式传输
  - 历史记录展示
  - 授权撤销后 token 失效
  - 客户端掉线中断
  - 敏感参数摘要脱敏

## 校验要求

- 实现阶段先执行相关 E2E 测试与 human_tests。
- 然后执行：
  - `cargo fmt --all -- --check`
  - `cargo clippy --workspace --all-targets --all-features -- -D warnings`
  - `cargo test --workspace --all-features`
  - `bash scripts/ci/local-ci.sh --skip-e2e` 或按影响面执行对应 E2E 套件
  - `rust-project-validate`

## 文档更新要求

- 实现时需要同步更新：
  - `README.md` 中的远程调用能力说明
  - WebUI / Admin API 文档
  - `human_tests/readme.md` 索引

## 已确认产品边界

1. 调用方按“纯程序化接入”设计，通过新增 `bifrost remote` 指令发起调用。
2. 首版命令范围固定为 Bifrost 只读查询命令，不支持任意 shell。
3. "永久授权"必须绑定调用方设备指纹（`caller_fingerprint`），而不是依赖会话级 token。
4. `Remote Invoke` 历史与授权管理放在 `Settings` 内，不新增一级导航。
5. 首版支持大流量传输，调用输入与输出都采用分片流式传输，不限制在中小载荷场景。
6. 首版支持多个 Bifrost 远端调用客户端同时在线，WebUI 在 `Settings -> Remote Invoke` 中统一管理多个客户端。
7. 首版支持多个远程调用方（不同 `caller_fingerprint`）同时对同一 Bifrost 客户端发起调用。各调用方的授权独立管理、会话密钥独立派生、执行互不干扰。
8. 授权流程采用蓝牙式"发现模式"：
   - Bifrost 客户端主动进入发现模式
   - 展示一个一次性授权码
   - `bifrost remote` 端使用该一次性授权码发起配对授权流程

---

## 附录：安全重构改造记录

> 本章节整合自 `design/remote-invoke-security-redesign.md`，记录 Remote Invoke 安全架构从初版到当前方案的核心变更。

### 改造背景

初版设计中 Caller 路由使用 `x-bifrost-token` 进行身份认证，但实际实现中 **所有 Caller 路由均无鉴权**。排查发现这并非遗漏，而是架构选择：在 Relay 透明中继模型下，Caller 不应持有 Relay 颁发的身份 token。

### 核心理念变更

| 维度 | 初版设计 | 当前方案 |
| --- | --- | --- |
| Relay 角色 | 身份提供者 + 中继 | 纯透明中继（不颁发 Caller 身份） |
| Caller 身份 | `x-bifrost-token` | 无身份概念，通过 `caller_fingerprint` 追踪 |
| 客户端发现 | `GET /clients` 枚举 | `pair_code` 一次性码（2 分钟 TTL） |
| 操作凭证 | Token + fingerprint | `grant_id` + `caller_fingerprint` 双因子 |
| relay_token | 会话级 token | per-call 临时路由令牌 |
| Grant 管理 | Caller 可 PATCH 修改 | Client 审批时确定，不可后续修改 |
| 数据隔离 | `user_id` 维度 | `client_instance_id` + `caller_fingerprint` 维度 |

### 安全门链

```
pair_code 可见性门控（6位码，2分钟TTL）
  → 人工审批（Client用户确认弹窗）
    → grant 绑定（client_instance_id + caller_fingerprint）
      → per-call relay_token（256-bit随机，call生命周期内有效）
        → 命令白名单（仅只读查询命令）
```

### 已删除/改造的端点

| 端点 | 处理方式 | 原因 |
| --- | --- | --- |
| `GET /clients` | 完全删除 | 防止客户端枚举，以 pair_code 替代 |
| `PATCH /grants/:id` (Caller 侧) | 完全删除 | Caller 不应修改 grant 属性 |
| `PATCH /client/grants/:grant_id` (Client 侧) | 新增 | Client 管理端可调整自己批准过的 grant 属性 |
| `GET /grants` | 完全删除 | Caller 无需列出所有 grant |
| `POST /pairings/start` | 改造 | 移除 `client_instance_id`（从 pair_code 解析）、`caller_pubkey`、`command` |
| `POST /calls/open` | 新增 | 独立的 call 创建端点，校验 grant 绑定后签发 relay_token |
| 所有 Caller 路由 | 移除鉴权 | Caller 无 token，安全性由 pair_code + grant 保障 |

### CLI 改造

- `CallerRelayClient` 结构中不再有 `token` 字段
- 新增本地连接文件 `{BIFROST_DATA_DIR}/remote-connections.json` 存储已建立的连接
- `resolve_local_connection()` 替代 `resolve_client_id()`，纯本地文件操作
- `caller_fingerprint` 生成算法（2026-06-16）：在 `{BIFROST_DATA_DIR}/remote-caller-identity.json` 中持久化 `caller-<32 hex chars>` 形态的随机 128 bit 值（`SystemRandom`），不再基于 username/hostname 派生
- 新增 SSH 公钥免配对授权链路（`bifrost remote conn up --ssh-key ...` / `/v4/remote-invoke/ssh/challenge` / `/v4/remote-invoke/ssh/connect`），与 6 位 pair_code 互斥可选
- 远端 caller 侧 `shared_secret` 通过 `{BIFROST_DATA_DIR}/remote-connections.key` 派生密钥使用 AES-256-GCM 加密落盘

### 实现状态标注（2026-06-16）

- ✅ Caller 路由无鉴权（透明中继模型）
- ✅ pair_code 发现机制（6位码，自动消费解析 client_instance_id）
- ✅ SSH 公钥免配对授权链路（pair_code 之外的第二种 AuthMethod）
- ✅ grant_id + caller_fingerprint 双因子操作凭证
- ✅ per-call relay_token 路由
- ✅ 本地连接文件与本地 caller 身份文件
- ✅ 命令白名单扩展：除只读查询外，已支持 `RemoteShellExec` / `RemoteShellInteractive` / `RemotePowerMgmt` / `RemoteImGateway` 等 `GrantScope`，并叠加独立的 `FileAccessScope`
- ✅ 端到端加密：`EncryptedEnvelope` v2 已承载真实密文，使用 X25519 + HKDF-SHA256 + ChaCha20-Poly1305 / AES-256-GCM
- ✅ Ed25519 challenge/response 注册签名验证（`invalid_registration_signature`）
- ⏳ `POST /v4/remote-invoke/client/token/renew` 续签端点（planned, not yet shipped as of 2026-06-16）
- ⏳ 设备指纹可信绑定与迁移流程（planned, not yet shipped as of 2026-06-16）
- ⏳ 审计 hash chain 与外部时间戳防篡改（planned, not yet shipped as of 2026-06-16）
