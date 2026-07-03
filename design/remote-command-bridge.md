# 远程调用桥接方案（Remote Command Bridge）

## 背景

Bifrost 需要一条“远端可调用本机命令”的通道：调用方（另一台设备、一个 CI runner、一个 Skill 后端）通过公网/内网中转，触发本机 Bifrost 客户端上的执行命令（初期只读查询，现已扩展到 shell/file/power/im 网关），并把结构化结果流式回传。为此设计了 Relay + Bifrost 客户端 + 本地 WebUI 三方架构，通过一次性配对码引导发现、人工授权确认，再由 grant 承载后续操作凭证。

落地路径分两个阶段：

- **本地验证阶段（shipped）**：Relay 实现在 `packages/bifrost-sync-server/` 单实例内存版本，可完整跑通 pair → grant → call → 大流量分片 → exit。
- **云端部署阶段（planned）**：将 Relay 迁移到 `bifrost-server-v4/app`，部署到 TCE 做组中/多实例测试。原文中大量 `bifrost-server-v4` 路径引用属于未来目标，非当前仓库结构。

统一协议模型不变：调用方 → Relay 走 HTTP + SSE；Relay → Bifrost 客户端走 SSE 长连接；Bifrost 客户端 → Relay 走 HTTP 上行事件。

## 用户目标验证清单

### 必须实现（已 ship / 部分扩展）

- 通过 Relay 中转触发本机 Bifrost 客户端命令。
- 首版仅支持只读查询命令（`status`、`traffic.list/get/search`、`search.get`）；现已扩展到 `RemoteShellExec`、`RemoteShellInteractive`、`RemotePowerMgmt`、`RemoteImGateway` 等 `GrantScope`，以及独立叠加的 `FileAccessScope`（`read/list/stat/glob/find/hash/write/edit/patch/mkdir/move/delete`），受 `FileAccessPolicy` 与 `GrantScope` 双重门控。见 `crates/bifrost-admin/src/handlers/remote_invoke.rs` 与 `crates/bifrost-admin/src/remote_invoke/types.rs`。
- 支持大流量结果流式传输与大载荷分片。
- 多个 Bifrost 远端客户端同时在线，统一在 `Settings → Remote Invoke` 管理。
- 多个远程调用方（不同 `caller_fingerprint`）并发调用同一 Bifrost 客户端，会话与授权隔离。
- 人工授权策略：一次调用 / 30 分钟 / 1 小时 / 1 天 / 永久（`GrantMode::{OnceCall, Ttl(30m/1h/1d), Permanent}`）。
- 一次性配对码：6 位数字，2 分钟 TTL，一次消费；同一客户端同时只允许一个 `pending_approval`。
- 双因子操作凭证：`grant_id`（UUID v4）+ `caller_fingerprint`（username+hostname hash）。
- per-call `relay_token` 临时路由，SSE 订阅隔离。
- 端到端加密：X25519 ECDH + HKDF-SHA256 + ChaCha20-Poly1305 / AES-256-GCM AEAD，Relay 只见密文。
- 多调用方配额与断路：`max_active_calls_per_client=5`（跨 caller 总数）、`max_active_calls_per_caller_per_client=3`、`max_grants_per_client=20`；超限返回 `call_limit_exceeded` / `grant_limit_exceeded`。

### 必须不破坏

- Caller 完全无身份 token 与登录概念，仅通过 `caller_fingerprint` 追踪。
- Relay 永不主动暴露注册客户端信息；`GET /clients` 等枚举入口已删除，只能通过 pair_code 发现。
- Client 注册仍需 `client_auth_token`（保持既有机制）。
- 现有本地 WebUI 授权 / 历史 / 撤销 / 事件流程签名不变。
- 大结果分片、加密握手、心跳恢复机制不因增量功能变化。
- Grant 一旦由 Client 审批签发即不可后续 PATCH 修改，Caller 也不能提权。

### 必须真实验证

- Web 上真实点击 “Enter discovery mode” → 生成 pair_code → CLI/程序端使用该码配对 → Web 弹窗人工审批 → CLI 端拿到 grant 与 `client_instance_id` → 发起 call 并收到 SSE 事件流 → 收到 `exit`。
- 撤销 grant 后 Caller 后续调用返回错误；SSE 连接被主动切断。
- 多 caller 同时对同一 client 并发跑 3 个以上 call，互不干扰、加密独立。
- 大文件 file API 分片写入完整；断线重连后未完成 call 可通过心跳 `active_call_ids` 恢复。

## 产品语义

### 四层安全模型

1. **配对层**：pair_code 是唯一发现机制。6 位数字仅用于让 caller 证明“知道当前可见的本地码”，不承担长期凭据。
2. **授权层**：Client 主动人工审批。审批通过才向 caller 释放 `client_instance_id`、设备信息、`grant`。
3. **凭证层**：`grant_id + caller_fingerprint` 双因子；Relay 校验 grant 有效期与吊销状态；per-call `relay_token` 只用于当次路由与 SSE 订阅。
4. **加密层**：调用内容与结果通过 caller/client ECDH 协商密钥端到端加密，Relay 不可解密。

### grant 生命周期

- 由 client 端审批时确定 `grant_mode` 与 `grant_scope`（后者含 `command_kinds` 白名单与可选 `file_access`）。
- Relay 校验 grant 时效（Once/30m/1h/1d/Permanent）。
- Client 侧可 WebUI 撤销；撤销后 Relay 拒绝后续 call 并推送 `grant_revoked` 事件。
- 可复用查询 `findReusableGrant(clientInstanceId, callerFingerprint)` 天然按 caller 隔离。

### 多 caller 并发

- 配对是**串行**的：新的 discovery 会取消旧的 `pending_approval` 并推 `pair_cancelled`；重复消费 pair_code 返回 `pair_code_already_consumed`；配对槽被占用返回 `pair_slot_occupied`。
- 授权是**并发**的：每个 caller 独立 grant；A 撤销不影响 B。
- 调用是**并发**的：多个 caller 可同时对同一 client 发 call；受 3 个配额约束。
- 客户端 SSE 下行复用同一条连接，通过 `call_id` 分发。

### 敏感参数脱敏

- 命令预览 / 审计中的参数字符串按敏感字段（`--token`, `password`, 环境变量前缀 `SECRET_` 等）脱敏为 `***`，见 `masked_summary_hides_sensitive_args` 单测约束。

## 技术细节

### 核心组件

| 组件 | 位置 |
| --- | --- |
| Bifrost 客户端执行侧 admin | `crates/bifrost-admin/src/handlers/remote_invoke.rs` |
| Grant/Scope/FileAccess/CommandKind 类型 | `crates/bifrost-admin/src/remote_invoke/types.rs` |
| 本地验证阶段 Relay | `packages/bifrost-sync-server/src/remote-invoke/{sse.ts,service.ts,cleanup.ts,ssh-auth.ts,pop.ts,types.ts}` |
| Caller CLI | `crates/bifrost-cli/src/commands/remote.rs`、`remote_grant.rs`、`remote_shell.rs`、`remote_ssh_key.rs`、`bifrost_file.rs` |
| WebUI Settings → Remote Invoke | `packages/bifrost-webui/src/pages/settings/RemoteInvoke*.tsx` |
| Grant/Call 审计 | Bifrost 本地 sled + Relay 端 store（sync-server 内存 Map，云端拟迁 Redis） |

### 关键 HTTP/SSE 端点（Relay 侧）

- `POST /api/remote-invoke/pairing` （Caller 提交 pair_code + fingerprint + 期望 grant_mode）
- `GET /api/remote-invoke/pairing/{pairing_id}/watch` (SSE)
- `POST /api/remote-invoke/pairing/{pairing_id}/approve|reject` （Client 审批）
- `POST /api/remote-invoke/call` （Caller 发起 call；Relay 生成 `call_id` + `relay_token`）
- `GET /api/remote-invoke/call/{call_id}/stream` (SSE, caller 侧)
- `POST /api/remote-invoke/client/upstream` （Client 侧上行 frame/exit 事件）
- `SSE /api/remote-invoke/client/watch` （Client 长连接接收下行事件）
- `POST /api/remote-invoke/grants/{grant_id}/revoke` （Client 侧撤销）

Client 侧（本地 admin API）：

- `POST /api/remote-invoke/discovery/start|stop`（生成/关闭 pair_code）
- `POST /api/remote-invoke/pairings/{id}/{approve|reject}`
- `GET /api/remote-invoke/grants` / `revoke`
- `GET /api/remote-invoke/calls?limit=N`
- `POST /api/remote-invoke/file/*`（受 `FileAccessPolicy` 与 `FileAccessScope` 双门控）

### CLI

- `bifrost remote pair <code>`：交互式配对，等待审批。
- `bifrost remote query status | traffic list | traffic get | traffic search | search get`：只读查询。
- `bifrost remote shell exec <cmd>` / `bifrost remote shell -i`：受 `RemoteShellExec` / `RemoteShellInteractive` 约束。
- `bifrost remote file read|list|stat|glob|find|hash|write|edit|patch|mkdir|move|delete`：受 `FileAccessScope` 与 `FileAccessPolicy` 双门控。
- `bifrost remote power ...` / `bifrost remote im ...`：受对应 GrantScope 约束。

### Sync 边界

- Relay 状态（pair、grant、call）不与规则/values sync 混合；Client 侧 grant 记录本地 sled，跨设备不同步。
- 加密密钥（ECDH ephemeral、AEAD session key）不落 Relay 端存储。
- Client 与 Relay 之间的 `client_auth_token` 仍走原有 sync-server 注册通道。

### 加密握手（简述）

1. Caller 提交 `pairing` 时附带 X25519 ephemeral public key。
2. Client 审批时用自己的 X25519 ephemeral 派生共享 secret，HKDF-SHA256 派生 AEAD key + nonce salt。
3. call 数据以 `envelope`（AEAD 加密的 payload + 8B seq + 16B tag）分片；每 chunk 递增 seq。
4. `envelope_encrypt_decrypt_roundtrip` 单测覆盖。

## Phase 1 – 本地闭环（shipped）

- sync-server 单实例内存 Map；完整 pair → approve → grant → call → frame → exit。
- CLI + WebUI + admin API 全部接入。
- 加密握手落地。

## Phase 2 – 命令白名单扩展（shipped）

- 从只读查询扩展到 `RemoteShellExec`、`RemoteShellInteractive`、`RemotePowerMgmt`、`RemoteImGateway`、`FileAccessScope`（细粒度动作 + `FileAccessPolicy` 路径/权限白名单）。
- Grant 审批时选择 scope；Relay 与 Client 双端强制校验。

## Phase 3 – 多 caller 并发与断路器（shipped）

- `max_active_calls_per_client=5`（总数）、`max_active_calls_per_caller_per_client=3`、`max_grants_per_client=20`。
- 配对串行、grant 并发、call 并发。
- 客户端心跳 `active_call_ids` 支持断线重连恢复。

## Phase 4 – 云端 Relay 迁移与监控（planned）

- 将 sync-server Remote Invoke 层迁到 `bifrost-server-v4/app`，多实例部署（配合 `redis-list-cross-instance-delivery.md`）。
- 落地下述监控指标与告警。
- 完成安全审计（`remote-invoke-security-redesign.md` 内容整合到本文档附录，见下）。

## 监控与告警（Phase 4 planned）

### Metrics

| 指标 | 类型 | 说明 |
| --- | --- | --- |
| `remote_invoke.pair_attempts_total` | Counter (labels: status=success/failed/expired) | 配对尝试总数 |
| `remote_invoke.pair_failure_rate_5m` | Gauge | 5 分钟滑动配对失败率 |
| `remote_invoke.active_grants` | Gauge | 当前活跃 grant 数 |
| `remote_invoke.active_calls` | Gauge | 当前活跃 call 数 |
| `remote_invoke.sse_connections` | Gauge (labels: role=caller/client) | SSE 连接数 |
| `remote_invoke.call_duration_seconds` | Histogram | 单次 call 时长 |
| `remote_invoke.relay_token_revocations_total` | Counter (labels: reason) | token 撤销总数 |
| `remote_invoke.encryption_failures_total` | Counter | 加密/解密失败 |

### 告警

| 告警 | 条件 | 严重级别 |
| --- | --- | --- |
| `HighPairFailureRate` | `pair_failure_rate_5m > 0.5` | Warning |
| `BruteForceDetected` | 单 IP 5m 内 pair 失败 > 20 次 | Critical，触发 15m IP 封禁 |
| `AbnormalCallerGeo` | 同一 fingerprint 短时多地理区域 | Warning |
| `SSEConnectionStorm` | 单 client_instance_id SSE 重连 > 30/min | Warning |
| `EncryptionFailureSpike` | encryption_failures_total 增长 > 10/min | Critical |

### 日志审计要求

- 配对失败必录 `caller_ip / client_instance_id / failure_reason / timestamp`。
- Grant 创建 / 撤销 / 过期完整审计链。
- 冷却与 IP 封禁走 `WARN` 结构化字段。

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

- Caller 通过一次性授权码配对 → 本地弹出授权。
- 拒绝后 caller 收到 `rejected`。
- 一次调用 grant：仅首个 call 成功，第二次 `grant_expired`。
- 30m/1h/1d/永久 grant：多次调用可复用。
- 客户端断线 → SSE 收到 `client_offline` / `interrupted`。
- 加密帧经 Relay 转发，服务端日志无明文。
- 多 caller 各自配对授权后可并发 call 同一 client。
- 多 caller grant 隔离：撤销 A 不影响 B。
- 并发 call 超 `max_active_calls_per_client` 返回 `call_limit_exceeded`。
- 90 天 / 10k 上限清理策略。

### 真实场景测试（human_tests/remote-invoke.md）

- 发现模式开/关与一次性授权码生成。
- 授权码一次性消费与过期。
- 全局弹窗授权 / 本次授权 / 30m/1h/1d/永久授权。
- 多客户端在线管理与选择。
- 多调用方并发配对与独立调用（两个不同 caller 同时查询同一 client）。
- 多调用方 grant 隔离撤销。
- 并发 call 超配额拒绝。
- 大结果流式传输 / 大文件分片。
- 历史记录展示。
- 授权撤销后 token 失效。
- 客户端掉线中断。
- 敏感参数摘要脱敏。

### 校验要求

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test --workspace --all-features`
- `bash scripts/ci/local-ci.sh --skip-e2e` 或按影响面执行对应 E2E 套件
- `rust-project-validate`

### 文档更新要求

- 更新 `README.md` Remote Invoke 能力说明与 CLI 帮助。
- 更新 WebUI 与 Admin API 文档。
- 更新 `human_tests/readme.md` 索引。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核 GrantScope + FileAccessScope 双门控在 handler 层是否短路（发现有一处走完加密再校验的路径需要提前）。
- 复核并发配对冲突：`pair_slot_occupied` vs `pair_code_already_consumed` 语义是否明确区分。
- 复核 grant 撤销后未消费的 pending call 是否立即 `grant_revoked` 并断流。
- 复测：所有列出的单测、`test_remote_invoke_e2e.sh`、human_tests 覆盖核心路径。

### 第 2 轮

- 复核 client 心跳 `active_call_ids` 断线恢复：Relay 是否会在旧 SSE 断开时丢弃 in-flight buffer，导致 caller 卡住。
- 复核审计日志字段稳定性（`grant_created/revoked/expired`、`call_open/exit`、`pair_failed`）。
- 复检监控指标与告警在 sync-server 模式下的采样入口（未来云端 Phase 4 迁移时保留）。
- 复测：多 caller 并发、大流量分片、加密握手回归；`envelope_encrypt_decrypt_roundtrip` + full e2e。

## 已确认产品边界

1. Caller 走“纯程序化接入”，通过 `bifrost remote` 指令与 SDK 发起调用。
2. 首版命令白名单仅只读查询；扩展的 shell/file/power/im 走独立 GrantScope，需 client 显式授权。
3. “永久授权”绑定 `caller_fingerprint`，不依赖会话级 token。
4. Remote Invoke 历史与授权在 `Settings` 内，不新增一级导航。
5. 支持大流量分片，无中小载荷限制。
6. 多 client 同时在线，`Settings → Remote Invoke` 统一管理。
7. 多 caller 并发独立授权、独立会话密钥、执行互不干扰。
8. 授权采用蓝牙式“发现模式” + 一次性授权码 + 人工审批。

## 附录：安全重构改造记录

本方案整合了 `design/remote-invoke-security-redesign.md` 的核心变更：

| 维度 | 初版设计 | 当前方案 |
| --- | --- | --- |
| Relay 角色 | 身份提供者 + 中继 | 纯透明中继（不颁发 Caller 身份） |
| Caller 身份 | `x-bifrost-token` | 无身份概念，通过 `caller_fingerprint` 追踪 |
| 客户端发现 | `GET /clients` 枚举 | `pair_code` 一次性码（2 分钟 TTL） |
| 操作凭证 | Token + fingerprint | `grant_id` + `caller_fingerprint` 双因子 |
| relay_token | 会话级 token | per-call 临时路由令牌 |
| Grant 管理 | Caller 可 PATCH 修改 | Client 审批时确定，不可后续修改 |
| 数据隔离 | `user_id` 维度 | `client_instance_id` + `caller_fingerprint` 维度 |

### 攻击面分析

| 攻击 | 防御 |
| --- | --- |
| 猜测 grant_id | UUID v4，128 位随机 |
| 窃取 grant_id | 需同时知道 `caller_fingerprint` |
| 伪造 caller_fingerprint | 需知目标 username+hostname |
| 遍历客户端 | `GET /clients` 已删除 |
| 重放过期 grant | Relay 校验时效 |

## 风险与决策点

- **多实例 Relay 尚未部署（Phase 4 planned）**：本地验证阶段 sync-server 单实例足以走通全链路；跨实例事件投递依赖 `redis-list-cross-instance-delivery.md`。
- **命令白名单扩张**：从只读查询扩到 shell/file 后攻击面提升；只能通过“Grant 审批时选择 scope + FileAccessPolicy 路径白名单 + 敏感参数脱敏 + 审计”联合把关。
- **Caller 无身份**：安全依赖“可见性管控 + 人工审批 + grant 双因子”；对被撤销的 caller 需要保证 SSE 立即断连。
- **端到端加密**：Relay 不解密调用内容，但握手参数 (ECDH 公钥) 会经过 Relay；密钥泄露风险靠 per-call ephemeral 与短生命周期缓解。
- **配对码熵**：6 位数字仅 10^6 空间；纯粹是“最近可见性证明”，加上速率限制、单次消费、串行槽、pair_slot_occupied 语义避免暴力枚举。
- **Cloud Relay 数据存储**：迁移到 `bifrost-server-v4/app` 后 pair/grant/call 状态可能需要持久化到 Alchemy Redis / MySQL；本方案不深入。
- **⏳ 未 ship**：
  - `POST /v4/remote-invoke/client/token/renew` 续签端点。
  - 设备指纹可信绑定与迁移流程。
  - 审计 hash chain 与外部时间戳防篡改。
