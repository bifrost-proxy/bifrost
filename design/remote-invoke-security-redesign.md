# Remote Invoke 安全重构方案

## 背景

Remote Invoke 早期实现把 Caller 路由暴露成了「透明枚举 + 弱鉴权」的形态：`GET /v4/remote-invoke/clients` 无鉴权即可列出 Relay 上所有在线 Bifrost 实例；`GET /grants` / `PATCH /grants/:id` / `DELETE /grants/:id` 都只需要知道 `grant_id` 就能操作；`POST /calls/open` 仅校验 `grant_id` 存在，不验证请求者与 grant 的绑定关系。这意味着任何人只要能访问 Relay，就能：

1. 列出所有客户端并按前缀猜出目标设备。
2. 用别人已经批准过的 `grant_id` 发起命令。
3. 篡改或注销别人的 grant。

设计文档 `design/remote-command-bridge.md` 6.1 节曾要求 `GET /clients` 加 `x-bifrost-token`、按用户隔离，但代码实现未落实。同时业务上又出现了一个反向需求：Caller 端不应该被强制要求登录 Relay，Relay 应该是「透明中继」——因此单纯给 Caller 路由加 token 并不合适，需要重构鉴权模型。

本方案在保持 Relay 透明中继语义的前提下，把安全模型收敛到「pair-code 门控 + grant + caller_fingerprint 双因子 + 命令白名单」四层闸门，并在此基础上落地了 X25519 端到端加密与 SSH key 复用配对两条演进路径。

> 实现校准（2026-06-16）：Relay 代码已整体迁移到 `packages/bifrost-sync-server`，原 `bifrost-server-v4/app/**` 路径不再存在；`caller_fingerprint` 已从 `simple_hash(username+hostname)` 升级为 16 字节随机数 `caller-<hex32>`，持久化于 `caller-identity.json`；pair-code 之外新增 SSH key 配对路径 `/ssh/challenge` + `/ssh/connect`，命令通过 `command_encrypted` 端到端加密，Relay 无法看到明文。

## 用户目标验证清单

### 必须实现

- 完全删除 `GET /v4/remote-invoke/clients` 端点，Relay 不再暴露枚举接口。
- 删除 Caller 侧 `GET /grants` / `PATCH /grants/:id`，只保留 Client 侧带 token 的 `client/grants/:id` PATCH。
- `POST /pairings/start` 只接受 `pair_code + caller_info + caller_ephemeral_pub`，由 Relay 从 pair_code 解析出 client_instance_id。
- `POST /calls/open` 校验 `caller_fingerprint === grant.caller_fingerprint` 且 `client_instance_id === grant.client_instance_id`，任一不匹配返回 403。
- `GET /grants/reusable` 与 `DELETE /grants/:id` 均需要 `caller_fingerprint` 参数，Relay 校验后再返回或删除。
- Caller 侧新增 `remote-connections.json` + `caller-identity.json` 两份本地文件，替换掉「无 token 枚举 + 前缀匹配」流程。
- `CallerRelayClient` 移除 `token` 字段和 `auth_headers()`，所有请求通过 URL / body 参数携带 `caller_fingerprint`。

### 必须不破坏

- Client 侧 `x-bifrost-token` 鉴权与 SSE 通道不变。
- 已发布的命令白名单（status / traffic.list / traffic.get / search.get / traffic.search）继续在 Relay + Client 双端校验。
- pair_code TTL、grant_mode 语义与执行流程不变。
- SSH key 复用配对路径与 pair-code 路径共用同一份 `remote-connections.json`。

### 必须真实验证

- Relay 上执行 `curl https://sync.example.com/v4/remote-invoke/clients` 返回 404。
- Caller 用错误 `caller_fingerprint` 调 `/grants/reusable` 或 `/calls/open` 返回 403。
- 同一台 Caller 反复 connect 同一 Client，新 grant 覆盖 `remote-connections.json`，旧 grant 自然过期。
- SSH key 路径 connect 后 `LocalConnection.auth_method == "ssh"`，`ssh_key_fingerprint` / `device_code` 字段被填。

## 产品语义

### 安全模型：pair-code + grant + fingerprint + 白名单

Relay 提供两根安全支柱：

1. **可见性管控**：Relay 不主动暴露注册的客户端信息。pair-code 是唯一的客户端发现机制（SSH key 路径要求 caller 事先持有对应设备的私钥，同样非枚举）。
2. **Client 主动授权**：只有被调用客户端弹窗审批通过后，才向 Caller 释放 `client_instance_id / grant_id / device_name / platform`。

四层闸门：

- `pair_code`：6 位、2 分钟 TTL、一次性；通过带外通道传递。
- 人工审批：Client 主人在 WebUI 弹窗看到 caller 的设备名、IP、平台后决定是否授权。
- `grant_id` + `caller_fingerprint`：UUID 不可暴力猜测 + 16 字节随机 fingerprint 绑定，非授权者拿到 grant_id 也无法使用。
- 命令白名单：Relay 与 Client 双端校验，即使 grant 有效也只允许 whitelisted 命令。

### Relay 是透明中继

Caller 端不需要注册 Relay、不需要登录 token；`grant_id + caller_fingerprint` 就是操作凭证。Relay 只做参数校验与转发，不承载 Caller 身份状态。

Client 端仍然需要 `x-bifrost-token` 完成 Relay 注册与 SSE 订阅——这是本机 Client 的能力（Bifrost 桌面/CLI 主体），不受 Caller 端改造影响。

### 双路径配对：pair_code + SSH key

- `pair_code`：一次性、面向临时授权、要求人工审批。
- SSH key：长期设备绑定、免去每次审批。签名挑战 `/ssh/challenge` + Ed25519 签名 `/ssh/connect`，Relay 完成公钥验签与 device_code 一致性校验后转发给 Client；Client 侧仍可选择自动批准或再次弹窗。
- 两路径共用后续的 grant / open_call / executor / SSE 结果通道。

## 技术细节

### Relay 路由变更（`packages/bifrost-sync-server/src/routes/remote-invoke.ts`）

| 端点 | 方法 | 变更 |
|------|------|------|
| `/v4/remote-invoke/clients` | GET | **删除** |
| `/v4/remote-invoke/grants` (Caller) | GET | **删除** |
| `/v4/remote-invoke/grants/:id` (Caller) | PATCH | **删除** |
| `/v4/remote-invoke/pairings/start` | POST | 移除 `client_instance_id / command / command_summary`，新增 `caller_ephemeral_pub` |
| `/v4/remote-invoke/grants/reusable` | GET | 强制要求 `caller_fingerprint`，缺失返回 400 |
| `/v4/remote-invoke/grants/:id` | DELETE | 强制要求 `caller_fingerprint`，不匹配返回 403 |
| `/v4/remote-invoke/calls/open` | POST | 校验 `caller_fingerprint` + `client_instance_id`，接受 `command_kind + command_encrypted` |
| `/v4/remote-invoke/calls/:id/events` | GET | 校验 call 归属（call 创建时已绑定 caller）|
| `/v4/remote-invoke/ssh/challenge` | POST | 新增：SSH key 领取签名挑战 |
| `/v4/remote-invoke/ssh/connect` | POST | 新增：Ed25519 签名 + `caller_ephemeral_pub` 发起配对 |
| `/v4/remote-invoke/ssh/connect-result` | POST | 新增：Client 上报审批结果 |

### 端到端加密

- `POST /pairings/start` 中 caller 携带 `caller_ephemeral_pub`（X25519 临时公钥）。
- `submitGrantDecision` 返回 `client_ephemeral_pub`。
- Caller 侧派生共享密钥并用 AEAD 加密 `command`，通过 `command_encrypted` 提交。Relay 无法解密，仅做路由。
- 缺少 `client_ephemeral_pub` 时 caller 会显式失败：`pairing succeeded but relay did not return client_ephemeral_pub required for encrypted remote commands`。

### Caller 本机文件

- `{BIFROST_DATA_DIR}/remote-connections.json`：`LocalConnection` 列表（`client_instance_id / device_name / platform / relay_url / grant_id / grant_mode / caller_fingerprint / connected_at / auth_method / ssh_key_* / device_code / transport_context_version / caller_ephemeral_pub / client_ephemeral_pub / shared_secret_encrypted`）。
- `{BIFROST_DATA_DIR}/caller-identity.json`：`caller_fingerprint` 长期身份。`generate_random_caller_fingerprint` 生成 16 字节随机数、格式化为 `caller-<hex32>`；`is_valid_caller_fingerprint` 校验必须以 `caller-` 前缀 + 32 位 hex。
- `{BIFROST_DATA_DIR}/remote-connections.key`：用于加密 `shared_secret_encrypted` 的本机对称密钥。

### CLI Caller 主流程（`crates/bifrost-cli/src/commands/remote.rs`）

```
1. Connect → handle_connect(pair_code) or handle_connect_ssh(--ssh-key)
2. 其他命令 →
   a. resolve_local_connection() 从 remote-connections.json 匹配 client
   b. find_reusable_grant(client_id, caller_fingerprint)
   c. open_call(grant_id, client_id, caller_fingerprint, command_encrypted)
   d. subscribe_call_events()
3. Disconnect →
   a. resolve_local_connection()
   b. DELETE /grants/:id?caller_fingerprint=Y
   c. 移除本地 LocalConnection
   d. --all 遍历本机所有连接
```

### Client Worker

无需改动 discovery/审批逻辑（`crates/bifrost-admin/src/remote_invoke/worker.rs`）。周边模块扩展：`executor.rs` / `file_ops.rs` / `file_access_roots.rs` / `file_policy_store.rs` / `grant_crypto_store.rs` / `grant_info_store.rs` / `grant_policy_store.rs` / `session_ring.rs` / `stream_emit.rs` / `ssh_keys.rs` / `identity.rs`，本方案不再展开。

## CLI + Web + Admin API

### CLI

```
$ bifrost remote connect <pair-code>
$ bifrost remote connect --ssh-key ~/bifrost-eden.key
$ bifrost remote status
$ bifrost remote exec --shell-text "..."
$ bifrost remote disconnect --client <prefix>
$ bifrost remote disconnect --all
```

### Web UI

- Settings → Remote Invoke → Discovery 面板生成 pair_code + QR。
- Grants 列表（走 Client 侧 token 化 API）展示已签发 grant，可撤销。
- SSH keys 管理面板（`packages/bifrost-sync-server` 侧无 UI，本地 Bifrost WebUI 由后续文档覆盖）。

### Admin API（本机 Bifrost 而非 Relay）

- `GET /_bifrost/api/remote-invoke/grants` — 列出本机已签发 grant（不参与本方案的 Caller 路径变更）。

## Sync 边界

- `remote-connections.json` / `caller-identity.json` / `remote-connections.key` 全部只属于本机 Caller，禁止 sync 到多设备。
- Relay 存储的 pair_code → client_instance_id 映射由 Client 心跳同步；SSH device_code → public_key 路由由 Client 主动登记，Relay 不做业务写入。
- `packages/bifrost-sync-server` 即是唯一 Relay 实现，不再存在本地 sync server 副本。

## Phase 1 – Relay 端裁剪

- 删除 `GET /clients` / `GET /grants` / `PATCH /grants/:id`。
- `POST /pairings/start` 精简入参并要求 `caller_ephemeral_pub`。
- `submitGrantDecision` 返回 `grant_scope / file_access / client_ephemeral_pub`。

## Phase 2 – Caller 端重构

- 引入 `remote-connections.json` + `caller-identity.json`。
- 删除 `resolve_client_id()`，新增 `resolve_local_connection()`。
- 简化 `CallerRelayClient`：移除 `token` / `auth_headers()`。

## Phase 3 – 端到端加密

- Caller 生成 X25519 临时 key pair，通过 `caller_ephemeral_pub` 提交。
- `command_encrypted` 替换明文 `command`；Relay 只转发。
- `/calls/:id/cancel` `/calls/:id/input` `/calls/:id/events` 使用 call-scoped 一次性 `relay_token`（不同于 caller 身份 token）。

## Phase 4 – SSH key 复用路径

- 新增 `SshAuthService` + `/ssh/challenge` + `/ssh/connect` + SSE `ssh_connect_result`。
- CLI `bifrost remote connect --ssh-key` + `bifrost setting ssh-key`。
- `LocalConnection.auth_method` = "ssh"，附带 `ssh_key_fingerprint / ssh_key_source / device_code`。

## 测试方案

### 单元测试

- `packages/bifrost-sync-server/src/__tests__/p0-hardening.test.ts` — 覆盖 `/clients` 404、`/pairings/start` 缺 `caller_ephemeral_pub` 拒绝、`/calls/open` fingerprint 不匹配拒绝。
- `packages/bifrost-sync-server/src/__tests__/remote-invoke-relay-v2-phase1.test.ts` — 覆盖 SSH challenge/connect 全链路。
- `packages/bifrost-sync-server/src/__tests__/remote-invoke-v5-test-utils.ts` — 提供共享 mock relay。
- `crates/bifrost-cli/src/commands/remote.rs` — 覆盖 `is_valid_caller_fingerprint` / `resolve_local_connection` / `handle_connect` 各分支。

### E2E 测试

- `e2e-tests/tests/test_remote_invoke_e2e.sh` — pair-code 主流程 + grant 有效性回归。
- `e2e-tests/tests/test_remote_invoke_ssh_e2e.sh` — SSH key 复用路径 + `remote exec --shell-text` + `remote traffic search` + `remote traffic get`。
- `e2e-tests/tests/test_remote_invoke_recent_calls_args_preview_e2e.sh` — Recent Calls 参数预览。
- `e2e-tests/tests/test_remote_connect_overload_retry_e2e.sh` — connect 过载重试。
- `e2e-tests/tests/test_remote_relay_url_fallback_e2e.sh` — relay URL 显式覆盖。
- `e2e-tests/tests/test_ssh_key_file_policy_migration.sh` — SSH key file policy 迁移。

### 真实场景测试 `human_tests/remote-invoke.md`

- pair_code 配对成功后 `remote-connections.json` 有一条记录。
- 错误的 `caller_fingerprint` 调用 `/grants/reusable` 返回 403。
- Caller 反复 connect 同一 Client，新 grant 覆盖旧记录。
- SSH key 路径连接成功后 `LocalConnection.auth_method == "ssh"`。
- `disconnect --all` 清空所有连接并在 Relay 侧撤销 grant。

## Review/Fix/Test 闭环

### 第 1 轮

- Review：路由层是否有遗漏的 Caller 路径未加 fingerprint 校验（grep `handleFindReusableGrant / handleOpenCall / handleRevokeGrant / handleSshConnect`）。
- Review：CLI 是否所有场景都走 `resolve_local_connection`，无残留 `resolve_client_id` 调用。
- Test：`p0-hardening.test.ts` + `test_remote_invoke_e2e.sh`。

### 第 2 轮

- Review：`caller_ephemeral_pub` / `client_ephemeral_pub` / `command_encrypted` 是否在 relay 层被落盘或日志泄露。
- Review：SSH key 撤销时 `device_code → public_key_pem` 路由是否同步删除。
- Test：`test_remote_invoke_ssh_e2e.sh` + `test_ssh_key_file_policy_migration.sh`。

## 风险与决策

- **无 caller 身份 token 的可接受性**：grant_id（128 位 UUID）+ 16 字节随机 fingerprint 双因子 = 256 位有效熵，足以抵御暴力猜测；Relay 不再暴露枚举接口，进一步压缩攻击面。
- **`caller_fingerprint` 随机化的可回溯性**：升级为随机数后失去了 `username@hostname` 的可读性，但换来了不可预测性。运维排查请依赖 `remote-connections.json` 里的 `device_name / platform` 字段。
- **不做向后兼容**：本方案属于 Breaking Change，需要客户端与服务端同步升级；由于产品尚未 GA，风险可控。
- **SSH key 复用与 grant cleanup 的耦合**：SSH key 本体独立管理，session grant 走 `remote-invoke-resilience.md` 中的 48h stale cleanup；SSH key 级默认 file access policy 不受影响。
- **Relay 端加密透明性**：Relay 无法读命令明文，若日志侧需要按命令做限流/审计，需要在 Client 侧完成后回传统计信息。
