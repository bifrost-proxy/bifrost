# Remote Invoke SSH 公钥鉴权扩展方案

## 背景

Remote Invoke 主链路已经稳定运行 `pair_code -> client approve -> grant -> openCall -> executor` 流程，配对与命令通道都有 grant / caller_fingerprint 双因子保护和命令白名单。但唯一的授权入口是 pair-code：每次连接都需要 Client 端人工弹窗审批，无法满足以下场景：

- CI / agent 沙箱要长期绑定单台目标 Client，反复要求人工审批不现实。
- Caller 换机、脚本化派发命令时，需要能带私钥自证身份、免人工确认。
- 多台 Caller 需要用同一份长期凭证连接同一台 Client。

本方案在不改动 pair-code 链路的前提下，新增 SSH 公钥鉴权作为并行 grant 签发路径。两条路径产出的 grant 完全兼容现有 openCall / executor / traffic 审计，后续调用链路只认 grant 不关心来源。安全模型仍遵循 `remote-invoke-security-redesign.md` 中的透明中继原则：Relay 只做公钥验签 + 设备 ID 一致性校验 + 路由转发，业务决策全部在 Client 端。

> 实现校准（2026-06-16）：本仓库已经落地 SSH 端到端链路。`crates/bifrost-admin/src/remote_invoke/ssh_keys.rs`（673 行）提供 `SshKeyStore` 与 `remote_invoke_ssh_keys` 表；CLI 在 `bifrost remote conn up --ssh-key ...` / `bifrost setting ssh-key ...` 入口可用；`packages/bifrost-sync-server/src/remote-invoke/ssh-auth.ts`（343 行）包含 `SshAuthService` + `/ssh/challenge` + `/ssh/connect` + `ssh_connect_result` SSE 全链路。`bifrost-server-v4` 与 WebUI 上的 SSH 密钥管理 UI 不在本仓库内，相关章节按 (planned, not yet shipped as of 2026-06-16) 对待。

## 用户目标验证清单

### 必须实现

- Caller 用 `bifrost remote conn up --ssh-key <keyfile>` 完成免审批连接。
- SSH key 与 pair-code 并行工作；同一 Client 可同时接受两种路径下的 grant。
- Relay 完成公钥验签 + `device_code` 一致性校验后再转发到 Client，防止路由投毒。
- Client 侧对 `ssh_key_fingerprint` 做二次校验，路由表被篡改时立即拒绝。
- SSH grant 一律 `grant_mode = permanent`，`grant_scope` 由密钥关联 policy 决定。
- 单密钥模型：每 Client 最多一条 `status=active` SSH 密钥，新建自动 revoke 旧密钥。
- 撤销密钥 → 删除 relay 路由 + 联动 revoke 所有 `auth_method=ssh_publickey` grant。

### 必须不破坏

- pair-code 主流程、grant 语义、命令白名单、`command_encrypted` 端到端加密不变。
- 老 grant 默认 `auth_method = pair_code`，兼容字段不缺。
- Relay 侧不新增业务决策；透明中继角色保持。
- SSH key 级默认 file access policy 与 `remote-invoke-resilience.md` 中的 grant cleanup 相互独立。

### 必须真实验证

- CLI 使用 `.bifrost` 密钥文件连接 Client，`LocalConnection.auth_method == "ssh"`，携带 `ssh_key_fingerprint / ssh_key_source / device_code`。
- Relay 端 `POST /ssh/connect` 收到非法签名返回 `ssh_signature_invalid`；device_code 与 payload 不一致返回 `device_code_mismatch`。
- Client 端二次确认失败（`ssh_key_fingerprint_mismatch` / `ssh_key_revoked`）直接拒绝，Caller 收到明确错误。
- 密钥重置后旧路由被删除、旧 grant 全部 revoke，新密钥立即可用。

## 产品语义

### 密钥文件是自包含设备身份

- 格式：`.bifrost` 自包含格式，含 `device_code` + Ed25519 私钥。
- `device_code` 派生算法：`BF-` + `hex(SHA256(public_key_der)[0..8])`，格式 `BF-XXXXXXXXXXXXXXXX`（16 位 hex，64 位熵）。
- Caller 从私钥即可推导公钥、独立计算 device_code，不需要额外配置。
- 密钥文件包含 device_code 是为了 CLI 免解析推导；Relay 侧仍会独立验证派生关系。

### Relay 是透明中继（防路由投毒）

Relay 只做三件事：

1. **公钥验签**：用路由表中的 `public_key_pem` 验证 Ed25519 签名。
2. **设备 ID 一致性校验**：签名 payload 中的 `device_code` 必须与请求 `device_code` 一致，防止攻击者用合法密钥冒充其他设备。
3. **路由到目标 Client**：验签通过后通过 SSE 转发给 `client_instance_id`。

Relay 只持有一张路由表 `device_code → { public_key_pem, client_instance_id }`。**所有 grant / policy / caller 记录都在 Client 端**。这保证即使 Relay 存储被入侵，也不会泄露业务数据。

### Client 二次确认

Relay 转发的 `ssh_key_fingerprint` 必须在 Client 侧再次比对本地 `remote_invoke_ssh_keys` 记录：

- 找不到对应 device_code → `ssh_key_not_found`
- 密钥 `status = revoked` → `ssh_key_revoked`
- fingerprint 不一致 → `ssh_key_fingerprint_mismatch`（路由可能被篡改）

只有二次确认通过才自动签发 grant。

## 技术细节

### Client 侧存储（`crates/bifrost-admin/src/remote_invoke/ssh_keys.rs`）

- `SshKeyStore` 封装 `remote_invoke_ssh_keys` 表操作。
- 表 schema：

  | 字段 | 类型 | 说明 |
  |------|------|------|
  | `id` | TEXT PK | UUID |
  | `device_code` | TEXT UNIQUE | `BF-XXXXXXXXXXXXXXXX` |
  | `label` | TEXT | 用户自定义标签 |
  | `public_key_pem` | TEXT | Ed25519 公钥 |
  | `ssh_key_fingerprint` | TEXT UNIQUE | `SHA256(public_key_der)` |
  | `private_key_pem_encrypted` | TEXT | AES-256-GCM 加密的私钥（仅 WebUI 复制分发用） |
  | `grant_mode` | TEXT | 固定 `permanent`（保留字段） |
  | `status` | TEXT | `active` / `revoked` |
  | `created_at` / `last_used_at` / `last_caller_info_json` | TEXT | 使用记录 |

- 单密钥模型：新建 key 时自动把已有 `active` key `revoke`，并推送新路由到 Relay。

### grant / call 表扩展

- `remote_invoke_grants` 新增字段：`auth_method` (`pair_code | ssh_publickey`)、`ssh_key_id`、`ssh_key_fingerprint`。
- `remote_invoke_calls` 新增字段：`auth_method`、`ssh_key_id`、`ssh_key_fingerprint`、`caller_info_json`。
- 老 grant 迁移时默认 `auth_method = pair_code`。

### Relay 侧路由表（`packages/bifrost-sync-server/src/remote-invoke/ssh-auth.ts`）

- Redis 键：
  - `ri:ssh_route:{device_code}` → `{ public_key_pem, client_instance_id }`，TTL 600s（跟随心跳）。
  - `ri:ssh_challenge:{challenge_id}` → `{ device_code, challenge, expires_at }`，TTL 120s。
- 生产集群 (`bifrost-server-v4`) 使用 `{device_code}` hash tag 保证同 slot。
- Client 通过 `POST /v4/remote-invoke/register` 与 `POST /v4/remote-invoke/heartbeat` 的 `ssh_device_route` 字段同步/续期路由；`syncSshRoute` 是唯一写入路径，同时负责校验 `device_code` 派生关系（防路由投毒）。

### 连接流程

```text
Caller
  → 加载 .bifrost 密钥文件 → 解析 device_code + 私钥
  → POST /v4/remote-invoke/ssh/challenge { device_code }
Relay
  → 查路由表确认 device_code 存在
  → 生成 64 字节 hex nonce + timestamp，Redis 存 challenge_id，TTL 120s
  → 返回 { challenge_id, challenge, expires_at }
Caller
  → 构造 payload（按 key 字母序排列 JSON）：
    {"challenge":"<nonce>","challenge_id":"<id>","device_code":"<code>","timestamp":<ms>}
  → Ed25519 签名 → base64
  → POST /v4/remote-invoke/ssh/connect { device_code, challenge_id, signature, timestamp, caller_info, caller_ephemeral_pub }
Relay
  → GETDEL challenge → 校验未过期
  → 校验 timestamp 在 ±30s 窗口
  → 查路由 → 用 public_key_pem 验签
  → 校验签名 payload 中的 device_code 与请求一致
  → 通过 SSE 转发到 Client：ssh_connect { connect_id, device_code, ssh_key_fingerprint, caller_info, relay_verified: true, caller_ephemeral_pub }
  → 立即返回 { connect_id }，Caller 打开 SSE 订阅 /caller-events?call_id=<connect_id>
Client
  → 二次确认（本地 ssh_keys 表 device_code + status + fingerprint 匹配）
  → 自动签发 grant：auth_method=ssh_publickey, caller_fingerprint=ssh_key_fingerprint, grant_mode=permanent
  → 生成 client_ephemeral_pub，派生共享密钥
  → POST /ssh/connect-result { connect_id, status: "authorized", grant_id, client_ephemeral_pub, ... }
Relay
  → pushToCallerStream(connect_id, 'ssh_connect_result', { grant_id, client_ephemeral_pub, ... })
Caller
  → SSE 收到 grant → 派生共享密钥 → 写入 remote-connections.json（auth_method="ssh", ssh_key_fingerprint / ssh_key_source / device_code / client_ephemeral_pub / shared_secret_encrypted）
  → 后续走 openCall 加密链路
```

### 异常码

- Relay 注册/心跳：`device_code_derivation_mismatch`
- Relay challenge：`device_code_not_found` / `challenge_rate_limited`（10 次/分钟）
- Relay connect：`challenge_expired` / `timestamp_out_of_window` / `ssh_signature_invalid` / `device_code_mismatch` / `client_offline` / `client_timeout`
- Client connect：`ssh_key_not_found` / `ssh_key_revoked` / `ssh_key_fingerprint_mismatch`
- `ssh_key_limit_exceeded`：单密钥模型下无 100 上限（planned, not yet shipped as of 2026-06-16 — 若未来允许多密钥再启用）

## CLI + Web + Admin API

### CLI

```
$ bifrost setting ssh-key create --label "CI Agent"
    → 输出 .bifrost 密钥文件路径 + device_code + fingerprint（仅此一次返回私钥）
$ bifrost setting ssh-key list
$ bifrost setting ssh-key reset
$ bifrost setting ssh-key delete
$ bifrost remote conn up --ssh-key ~/bifrost-ci.key --relay-url https://sync.example.com
$ bifrost remote conn status
$ bifrost remote exec --shell-text "..."
```

### Web UI（planned, not yet shipped as of 2026-06-16）

- Settings → Remote Invoke → SSH Keys 面板管理 active 密钥。
- 新建后弹窗展示 `.bifrost` 密钥文件下载入口（仅此一次）。
- 重置密钥需二次确认，明确提示会 revoke 旧路由与旧 grant。

### Admin API（本机 Bifrost）

- `POST /_bifrost/api/remote-invoke/ssh-key`：创建（同步新路由到 Relay）。
- `GET /_bifrost/api/remote-invoke/ssh-key`：返回当前 active 密钥。
- `GET /_bifrost/api/remote-invoke/ssh-key/private-key`：再次获取（弹窗确认后）。
- `POST /_bifrost/api/remote-invoke/ssh-key/reset`：原子替换。
- `DELETE /_bifrost/api/remote-invoke/ssh-key`：撤销。
- `PATCH /_bifrost/api/remote-invoke/ssh-key`：更新 label 等元数据。

### Relay API

- `POST /v4/remote-invoke/ssh/challenge` — 签发挑战。
- `POST /v4/remote-invoke/ssh/connect` — 验签 + 转发。
- `POST /v4/remote-invoke/ssh/connect-result` — Client 上报审批结果（Relay pushToCallerStream 到 caller SSE）。

## Sync 边界

- `remote_invoke_ssh_keys` 表属于本机 Client，禁止 sync 到多设备（私钥不应离开本机）。
- Relay 路由表由 Client 主动同步，Client 撤销即 Relay 撤销。
- Caller 端 `remote-connections.json` 也是本机数据，不 sync。
- Client 侧密钥重置 → Relay `syncSshRoute` 原子替换旧路由 + 本地 revoke 关联 grant → 生产 Relay 侧同步撤销 `auth_method=ssh_publickey` grants（避免旧密钥残留复用）。

## Phase 1 – Client 密钥存储与生成

- `SshKeyStore` 表结构 + CRUD。
- `.bifrost` 密钥文件格式与解析。
- 密钥重置流程（自动 revoke 旧、通知 Relay、清 grant）。

## Phase 2 – Relay 验签与路由

- `SshAuthService.syncSshRoute` / `issueChallenge` / `verifyAndConnect`。
- `POST /v4/remote-invoke/register` / `POST /v4/remote-invoke/heartbeat` 携带 `ssh_device_route`。
- Ed25519 验签 + device_code 一致性校验。

## Phase 3 – 连接 SSE 与 grant 签发

- 复用 `registerCallerEventStream` / `pushToCallerStream` 投递 `ssh_connect_result`。
- Client 二次确认 + 自动签发 permanent grant。
- 携带 `caller_ephemeral_pub` / `client_ephemeral_pub`，完成共享密钥派生。

## Phase 4 – CLI + Admin API + WebUI

- CLI `bifrost setting ssh-key` 子命令 + `bifrost remote conn up --ssh-key`。
- Admin API `POST/GET/PATCH/DELETE /_bifrost/api/remote-invoke/ssh-key`。
- WebUI SSH keys 管理面板（planned, not yet shipped as of 2026-06-16）。

## 测试方案

### 单元测试

- `crates/bifrost-admin/src/remote_invoke/ssh_keys.rs` — `SshKeyStore` 单密钥模型、撤销联动、fingerprint 唯一性。
- `packages/bifrost-sync-server/src/__tests__/remote-invoke-relay-v2-phase1.test.ts` — SSH challenge/connect/connect-result 全链路 mock 覆盖。
- `packages/bifrost-sync-server/src/__tests__/p0-hardening.test.ts` — `device_code_derivation_mismatch` / `ssh_signature_invalid` / `device_code_mismatch` / `challenge_expired` 拒绝路径。
- `packages/bifrost-sync-server/src/__tests__/remote-invoke-v5-test-utils.ts` — 共享测试工具。

### E2E 测试

- `e2e-tests/tests/test_remote_invoke_ssh_e2e.sh` — SSH key 复用连接 + `remote exec --shell-text` + `remote traffic search|get`。
- `e2e-tests/tests/test_ssh_key_file_policy_migration.sh` — SSH key 关联的默认 file access policy 迁移与保留。

### 真实场景测试

- `human_tests/remote-invoke-sshkey.md` — SSH key 创建、连接、重置、撤销全流程。
- `human_tests/remote-invoke-file.md` — SSH key 关联 file access policy 场景。
- `human_tests/remote-invoke.md` — SSH grant 与 pair-code grant 并存断言。
- `human_tests/remote-shell-exec.md` — SSH key 走 shell exec 白名单。

### 校验要求

按顺序执行：

1. `cargo test -p bifrost-admin ssh_keys`
2. `pnpm --filter bifrost-sync-server test remote-invoke-relay-v2-phase1 p0-hardening`
3. `bash e2e-tests/tests/test_remote_invoke_ssh_e2e.sh`
4. `bash e2e-tests/tests/test_ssh_key_file_policy_migration.sh`
5. `bash human_tests` 中 SSH key 相关用例手工复跑

## Review/Fix/Test 闭环

### 第 1 轮

- Review：Relay 验签是否严格用 `SshAuthService` 的路由表公钥，不接受请求中携带的 pubkey。
- Review：Client 二次确认是否覆盖 `ssh_key_not_found / ssh_key_revoked / ssh_key_fingerprint_mismatch`。
- Test：SSH E2E 全链路 + `p0-hardening.test.ts` 拒绝路径。

### 第 2 轮

- Review：密钥重置流程是否原子——旧路由删除 → 新路由写入 → 旧 grant revoke 全部完成后才返回 200。
- Review：`caller_ephemeral_pub` / `client_ephemeral_pub` 是否在 Relay 侧只做透传，不落盘。
- Test：`test_ssh_key_file_policy_migration.sh` + human_tests 手动 SSH 用例。

## 风险与决策

- **私钥托管风险**：Bifrost 帮用户生成私钥并保留 AES-256-GCM 加密副本供 WebUI 复制分发；用户必须妥善保管密钥文件，一旦泄露就等价于设备被盗。撤销机制作为止损手段：`DELETE /api/remote-invoke/ssh-key` 立即 revoke 路由与 grant。
- **路由投毒防御**：Relay 在 `syncSshRoute` 时独立校验 `device_code = BF- + hex(SHA256(public_key_der)[0..8])`；`/ssh/connect` 时用路由表公钥验签且比对签名 payload 中的 `device_code` 与请求一致。三层防御要同时通过。
- **单密钥 vs 多密钥**：第一版采用单密钥模型（每 Client 一条 active key），大幅简化 UI 与撤销边界；未来若需要多密钥（例如临时授权/团队共享），需要重新设计 UI 与限流。
- **grant TTL**：SSH grant 一律 permanent；如果需要临时授权仍走 pair-code。这样保持两条路径的语义清晰。
- **Relay 双版本一致性**：`packages/bifrost-sync-server` 与 `bifrost-server-v4` 共享同一 HTTP 契约，但存储实现不同；对外的生产 Relay 必须额外满足「持久化 SSH grant + `grant_idx(client_instance_id)` 写入 + 密钥撤销时清理 `ssh_publickey` grant」三条要求，避免行为漂移。
- **CI 环境限流**：challenge 端点限流 10 次/分钟；CI 高并发场景需要在测试脚本里控制串行度，避免误命中 `challenge_rate_limited`。
