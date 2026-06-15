# Remote Invoke SSH 公钥鉴权扩展方案

> 状态：决策已确认 | 更新时间：2026-04-22

## 背景

当前 Remote Invoke 主链路已经具备：

- `pair_code -> client approve -> grant -> openCall -> executor`
- 基于 `grant` 的复用、过期、调用次数与客户端归属校验
- relay 与 client 双端命令白名单

但现状仍有一个明显短板：调用方只能通过授权码配对，缺少更适合长期设备绑定的 SSH 公钥鉴权方案。

本方案在不推翻现有授权码链路的前提下，新增 SSH 公钥授权，作为授权码的并行 grant 签发路径。

## 方案结论

### 1. 授权码与 SSH 并行，不互相依赖

二者都只是 grant 的签发方式：

```text
pair_code   -> client approve -> grant(pair_code)
ssh key     -> device_code + sign -> relay route -> client auto grant(ssh_publickey)
grant -> openCall -> call_open -> executor
```

后续调用链路统一只认 grant，不关心 grant 的来源。

### 2. Relay 是透明的薄路由层

**核心范式**：Relay 不做任何业务决策，只做三件事——**公钥验签**、**设备 ID 一致性校验** 和 **路由到目标 Client**。

> **安全动机（防路由投毒）**：如果 Relay 仅做路由转发而不做任何验证，一旦路由表被投毒（攻击者篡改 `device_code → client_instance_id` 映射或注入伪造路由），Relay 会将未经验证的恶意数据无脑转发给 Caller，导致 **Caller 被投毒**——接收到伪造的 grant、错误的 Client 响应、甚至被中间人劫持。因此 Relay 必须在转发前完成公钥验签 + 设备 ID 一致性校验，确保请求来自合法的密钥持有者且路由目标与签名身份一致，从源头阻断投毒数据的传播链路。

Relay 的全部存储仅为一张路由表：

```text
device_code → { public_key_pem, client_instance_id }
```

这张路由表在 Client 注册/心跳时由 Client 主动同步上来。Relay 不存储 grant、scope、policy、密钥详情、caller 信息等任何业务数据。

所有业务决策（grant 签发、policy 执行、caller 信息记录）全部在 Client 端完成。

### 3. device\_code：公钥的确定性派生标识

`device_code` 是从 SSH 公钥确定性派生的永久设备码，**嵌入在密钥文件中**：

- 派生算法：`BF-` + `hex(SHA256(public_key_der)[0..8])`，格式 `BF-XXXXXXXXXXXXXXXX`（16 位 hex，64 位熵）
- 由于 Ed25519 私钥可推导公钥，因此 **caller 从私钥即可计算出 device\_code**
- 密钥文件中预嵌入 device\_code，caller 直接解析无需计算
- Relay 路由表的 device\_code 由 Client 注册时上报（Client 从公钥派生）
- **Relay 必须验证 device\_code 的派生关系**：注册时从 public\_key\_pem 独立计算 device\_code，与声称的值比对
- 永久有效，不会自动过期，与动态 `pair_code` 完全不同

> **安全设计决策**：使用 8 字节（64 位）而非 4 字节（32 位），理由：32 位仅有 \~43 亿种可能，Ed25519 密钥生成速度极快（数十万对/秒），暴力碰撞可在数分钟内完成；64 位将碰撞成本提升到 \~1.8×10^19，实际不可行。

```text
派生链：
  公钥 → SHA256(public_key_der) → 取前 8 字节 → hex → BF-XXXXXXXXXXXXXXXX
  私钥 → 推导公钥 → 同上

对比：
  pair_code    = 动态、一次性、几分钟有效、用于人工配对
  device_code  = 静态、永久、确定性派生自公钥、64 位熵、嵌入密钥文件
```

### 4. SSH 密钥由被控端生成，分发即授权

密钥在 Client WebUI 生成，admin 只需分发 **一个 Bifrost 密钥文件** 给 caller → 持有即授权。

- 被控端在 WebUI 生成 Ed25519 密钥对，device\_code 从公钥自动派生
- WebUI 提供 Bifrost 格式密钥文件的一键复制/下载（内含 device\_code + 私钥）
- caller 只需这一个文件即可连接：解析出 device\_code 用于路由 + 私钥用于签名
- 无需额外审批，无需单独记忆 device\_code
- 被控端 WebUI 展示连接信息（caller hostname、IP、platform），仅用于监控
- 被控端可随时在 WebUI 撤销密钥，relay 路由表同步删除

### 5. Bifrost 密钥文件格式

定义自包含的 Bifrost 密钥文件格式，将 device\_code 与私钥打包在一个文件中：

```text
-----BEGIN BIFROST KEY-----
Device-Code: BF-A1B2C3D4E5F6A7B8
<base64-encoded Ed25519 private key>
-----END BIFROST KEY-----
```

设计要点：

- **自包含**：caller 只需一个文件，不需要额外参数
- **可解析**：header 中的 `Device-Code` 字段可直接提取，无需加密运算
- **可验证**：caller 可从私钥推导公钥，再计算 device\_code 与 header 交叉验证
- **兼容性**：CLI 也支持直接传入标准 Ed25519 私钥文件 + 自动计算 device\_code（无 header 时 fallback）

## 2026-04-21 CI 跟进

### SSH connect 的短暂重连窗口

在 `reset ssh key -> worker reconnect -> ssh/challenge -> ssh/connect` 这条链路里，路由表通常会先于 client SSE stream 恢复。如果 relay 在 `verifySshConnect()` 中只做一次 `pushToClient()`，就可能在极窄窗口里把本应成功的请求误判成 `client_offline`。

因此本轮修复追加一个轻量补偿策略：

- `/v4/remote-invoke/ssh/connect` 验签成功后，若第一次 `pushToClient()` 失败，不立即返回 `client_offline`
- relay 额外等待一个很短的窗口（秒级、轮询 `getClientStream()`）
- 一旦发现 client stream 已恢复，立即重试投递 `ssh_connect`
- 只有在补偿窗口结束后仍无在线 stream，才返回 `client_offline`

这个策略不改变鉴权协议，也不改变 grant 语义，只是把 reset/reconnect 场景里的瞬时时序抖动从“硬失败”收敛为“短暂等待后成功”。

### 本轮验证计划

- 单元/类型侧：
  - 保持 `ssh/challenge` 与 `ssh/connect` payload 不变，避免引入协议不兼容
- E2E：
  - 运行 `e2e-tests/tests/test_remote_invoke_ssh_e2e.sh`
  - 重点覆盖 reset 后重新申请 challenge、签名 connect、等待 `ssh_publickey` grant 出现

## 2026-04-28 Caller ID 隔离修复

### 问题

SSH key connect 之前把 SSH key fingerprint 复用为 `caller_fingerprint`，pair-code connect 也曾基于 `username + hostname` 派生 caller fingerprint。这会让同一机器、同一沙箱模板、或同一把 SSH key 在多个 caller 沙箱中反复连接时命中同一个 caller identity。

Relay 的 reusable grant 索引是 `{client_instance_id, caller_fingerprint}`。当多个 caller 共享同一个 fingerprint 时，新 SSH connect 会撤销/覆盖同 caller 的旧 active grant，表现为多个 caller 竞争同一 target grant。

### 方案

- CLI caller 在 `{BIFROST_DATA_DIR}/remote-caller-identity.json` 中保存随机生成的 `caller_fingerprint`，格式为 `caller-<128-bit-hex>`。
- caller ID 首次使用时生成，同一 `BIFROST_DATA_DIR` 内持久复用；不同沙箱/数据目录自然生成不同 ID。
- `CallerInfo.display_name` / `hostname` / `username` 继续优先使用系统信息，只作为可读展示名称，不参与 relay grant 归属。
- SSH key fingerprint 只表示“这把 key 被 target 信任”，继续写入 `ssh_key_fingerprint` 字段，用于审计、展示和 file policy 的 `match.ssh_fingerprint`。
- target 处理 `ssh_connect` 时用 `caller_info.fingerprint` 作为 grant 的 `caller_fingerprint`；仅当旧 caller 未发送 fingerprint 时，才兼容回退到 SSH key fingerprint。
- relay 存储 SSH grant 时按随机 caller ID 建 reusable 索引，同时保留独立 `ssh_key_fingerprint` 字段；`call_open` 下发时也携带 `ssh_key_fingerprint`，方便 target 在本地 grant 恢复路径继续识别 SSH 授权。

### 测试方案

## 2026-05-26 本机 CLI 管理 SSH Key

### 目标

补齐 WebUI 之外的本机管理入口，让 target 机器可以用 CLI 快速生成并导出 remote-invoke SSH key 文件，再交给 caller 机器执行：

```bash
bifrost remote conn up --ssh-key ./bifrost-device.key
```

### 命令设计

新增 `bifrost setting ssh-key ...`，归属 `setting` 而不是 `remote`：

- `setting` 始终管理当前机器的数据目录，符合“在被控端生成 key”的安全模型。
- `remote` 是 caller 操作另一台机器的入口，只消费 `--ssh-key`，不负责生成 target key。

子命令：

```bash
bifrost setting ssh-key create --label dev-mac --output ./bifrost-device.key
bifrost setting ssh-key export --output ./bifrost-device.key --force
bifrost setting ssh-key status
bifrost setting ssh-key revoke
```

实现策略：

- 默认优先调用本机正在运行的 Admin API：`/_bifrost/api/remote-invoke/ssh-key`，让 worker 立即刷新 relay route。
- 如果 Admin API 不可达，回退到直接写本机 `BIFROST_DATA_DIR` 下的 `SshKeyStore`；服务下次启动时会读取该 active key。
- `create` 生成/替换 active key，输出 Bifrost key file；`--output` 写文件时使用 `0600` 权限并默认拒绝覆盖。
- 离线生成时同步写入默认 SSH file-access policy，保持与 WebUI 创建 key 后默认可用的行为一致。

### 测试方案

- 单元测试：
  - `parse_grant_mode` 覆盖 `once/30m/1h/1d/permanent`。
  - `write_secret` 覆盖不带 `--force` 拒绝覆盖，以及 Unix 下 key 文件权限为 `0600`。
  - Admin API fallback 只在连接不可达错误时触发。
- E2E：
  - 新增 `e2e-tests/tests/test_setting_ssh_key_cli.sh`，使用临时 `BIFROST_DATA_DIR` 验证 `create/export/status/revoke`。
  - 验证生成文件包含 `BEGIN BIFROST KEY` 和 `Device-Code`，且 `remote conn up --ssh-key <file> --device-code BF-...` 能解析 key 并进入 relay challenge 路径。
  - 将脚本加入 `scripts/run_all_e2e.sh` 的 stable shell test 集合；CI `--ci` / `--full-shell` 模式会通过 `find e2e-tests/tests/test_*.sh` 自动收集，stable 模式也会显式执行。
- human_tests：
  - 更新 `human_tests/remote-invoke-sshkey.md`，新增本机 CLI 生成 key 的真实场景用例。

### Review/Fix/Test 闭环方案

- 第 1 轮：复核 `setting` 命名、API 优先/离线 fallback、文件权限、文档和 E2E 覆盖；运行 CLI 单测与新增 E2E。
- 第 2 轮：复核生成 key 是否能被 caller 侧 loader 接受、human_tests 索引是否同步、输出是否适合 shell 重定向；复跑受影响测试。

## 2026-06-02 Caller 侧固定环境变量读取 SSH Key

### 目标

让 CI/自动化 caller 无需把 SSH key 写入本地文件，也无需记忆任意 `env:NAME` 语法。`--ssh-key` 本身表示启用 SSH key 授权：

```bash
export BIFROST_REMOTE_SSH_KEY="$(cat ./bifrost-device.key)"
bifrost remote conn up --ssh-key --label ci-agent
```

### 命令语义

- `bifrost remote conn up <pair-code>`：继续使用一次性 pair code。
- `bifrost remote conn up --ssh-key <path>`：从指定 key 文件读取。
- `bifrost remote conn up --ssh-key`：从固定环境变量 `BIFROST_REMOTE_SSH_KEY` 读取。
- `env:NAME` 不再作为可自由配置的用户入口；只允许固定 `env:BIFROST_REMOTE_SSH_KEY`，避免不同脚本使用不同环境变量名导致运维不可复用。

### 错误提示

- `--ssh-key` 无值且 `BIFROST_REMOTE_SSH_KEY` 未设置：提示设置固定环境变量或传 `--ssh-key <path>`。
- `BIFROST_REMOTE_SSH_KEY` 为空：提示设置非空值或传文件路径。
- `env:OTHER`：明确提示只支持固定 `env:BIFROST_REMOTE_SSH_KEY`。

### 测试方案

- 单元测试：覆盖 clap 将 `--ssh-key` 无值解析为固定 env source；覆盖 env 正常读取、缺失、空值、转义换行还原和非固定 env 名拒绝。
- E2E：`test_setting_ssh_key_cli.sh` 使用固定 env 验证 caller parser；`test_remote_invoke_ssh_e2e.sh` 保留第一 caller 文件模式，并让第二 caller 通过 `BIFROST_REMOTE_SSH_KEY` 完成真实 SSH connect。
- human_tests：更新 `remote-invoke-sshkey.md`，新增固定 env 用例并记录执行结果。

## 2026-06-15 SSH key 默认 Full Trust

### 问题

`bifrost setting ssh-key create` 只会生成 key 并 seed 文件访问策略；SSH key 连接时 target 端调用 `shell_grant_provision(None, None, None, None, None)`。如果目标设备尚未配置任何 enabled Shell Access policy，`shell_grant_provision` 会降级为 `remote_query`。caller 之后执行 `bifrost remote exec` 会触发 `grant_scope_mismatch`，即使 CLI 自动重新 SSH 授权，也仍然拿到只读 grant。

### 决策

SSH key 是“分发即授权”的长期设备绑定方式，默认必须是 Full Trust：caller 持有 key 后可以运行命令、读写文件、发送 stdin、打开交互式终端并查看状态/流量。只有 pair/code 的交互式授权流程允许用户按需选择 `Read-only watch`、`Files only`、`Run commands & read/write files`、`Full trust` 或 `Custom`。

### 实现

- storage 层新增内置 `ssh-key-full-access` Shell Access policy：允许 `argv_exec` 和 `shell_text`、任意 executable、任意 shell text、stdin、interactive、继承环境。
- SSH key create/reset/offline create 以及 SSH connect 前都会确保该 policy 存在且启用。
- SSH key 自动 grant 固定生成为 `remote_shell_interactive + file_access=read_write + stdin_allowed=true + interactive_allowed=true`，并以 `selected[ssh-key-full-access]` 绑定，避免 `mode=all` 与其他 enabled policy 同时匹配时产生 ambiguous。
- `bifrost setting grant update` 新增权限级别入口：`--level full|shell|files|query`，并支持缺省交互式选择 grant/device 与权限级别。底层 `--access/--scope/--file-access` 参数仍保留给高级用法。
- WebUI 的 `Full trust` / `Run commands & read/write files` preset 绑定同一内置 full-access policy；`Custom` 仍可选择普通 Shell Access policy。

### 测试方案

- 单元测试：
  - storage：`ensure_default_ssh_key_shell_policy` 创建、幂等、保留既有 policy。
  - CLI：`setting grant update --level` 解析和 level payload 映射。
- E2E：
  - `e2e-tests/tests/test_remote_invoke_ssh_e2e.sh` 在 SSH key connect 后执行 `remote exec --shell-text "printf ssh-full-trust-ok"`，验证 saved SSH grant 默认可运行 shell。
- human_tests：
  - 更新 `human_tests/remote-invoke-sshkey.md`，新增 SSH key 默认 Full Trust 回归用例并执行。

- 单元测试：
  - `test_random_caller_fingerprint_has_expected_shape` 验证随机 caller ID 格式。
  - `test_load_or_create_caller_fingerprint_persists_per_data_dir` 验证同一数据目录复用。
  - `test_load_or_create_caller_fingerprint_differs_across_data_dirs` 验证不同数据目录不重复。
  - `test_recover_call_open_ssh_file_grant_preserves_file_access` 验证随机 caller ID 与独立 SSH fingerprint 能同时恢复 SSH grant 语义。
- E2E：
  - 更新 `e2e-tests/tests/test_remote_invoke_ssh_e2e.sh`，使用同一个 exported SSH key 从两个 caller 临时数据目录连接同一 target。
  - 断言两个 caller 的 `remote-connections.json` 中 `caller_fingerprint` 均为 `caller-*` 且互不相等。
  - 断言 target grants 中存在两条 `ssh_publickey` grant：`ssh_key_fingerprint` 相同，但 `caller_fingerprint` 分别匹配两个 caller。
- 真实场景测试：
  - 新增 `human_tests/remote-invoke-sshkey.md` 的 `TC-RISK-01`，按脚本逐条执行并确认多 caller grant 不再互相覆盖。
- human_tests：
  - 更新 `human_tests/remote-invoke.md`
  - 重新执行 `TC-RI-回归-104`、`TC-RI-回归-105`，确认 reset 后的首轮 SSH connect 不再因为短暂离线窗口失败

## 设计原则

### 1. SSH key = caller identity（无需本地持久化）

**旧方案问题**：CLI 的 `caller_fingerprint` 依赖 `hostname + user + DefaultHasher`，不稳定；即使改为本地持久化 `remote_caller_identity.json`，在临时沙箱中也会随环境销毁而丢失。

**新方案**：SSH 密钥本身就是身份。

- `caller_fingerprint = ssh_key_fingerprint`（即 SSH 公钥的 SHA-256 指纹）
- 只要 caller 持有同一把私钥，无论在哪台机器、哪个沙箱，身份都是稳定的
- 私钥可以存储在 CI secret、环境变量、密钥管理服务中

```text
身份链：
  Bifrost 密钥文件（caller 持有）
    → 解析出 device_code（路由标识）
    → 解析出私钥
      → 推导公钥（被控端存储）
        → fingerprint = sha256(public_key)
          → caller_fingerprint（用于 grant 绑定与审计）
  device_code（从文件解析 or 从公钥派生）
    → relay 路由表查找 → client_instance_id
```

### 2. Relay 最小化存储（透明路由 + 防投毒验证）

> **为什么 Relay 必须做验证？** Relay 路由表是攻击面最大的单点——一旦被投毒（篡改映射、注入伪造路由），如果 Relay 不做任何验证就转发，Caller 将直接接收到恶意数据（伪造 grant、中间人响应等），整条信任链从源头崩塌。因此 Relay 在保持"不做业务决策"的前提下，必须完成**最小必要验证**：确认请求确实来自合法密钥持有者，且请求身份与路由目标一致。

Relay 的职责严格限定为：

1. **存储路由表**：`device_code → { public_key_pem, client_instance_id }`
2. **公钥验签**（防伪造请求）：用路由表中匹配设备的 public\_key 解码验证 caller 签名的数据，确保请求来自合法密钥持有者
3. **设备 ID 一致性校验**（防路由投毒/设备冒充）：从已验证的签名 payload 中提取 device\_code，与请求声称的 device\_code 比对，不一致则拒绝——即使路由表被篡改，攻击者也无法将合法签名绑定到错误的路由目标
4. **路由转发**：上述校验全部通过后，才将请求转发给 client\_instance\_id 对应的 Client（Client 再做二次确认，形成纵深防御）

Relay **不做**的事情：

- 不存储 grant、scope、policy
- 不决定是否签发 grant
- 不记录 caller 连接历史
- 不做命令白名单校验
- 不存储密钥详情（label、status、创建时间等）

路由表由 Client 在注册/心跳时主动上报（每个 Client 至多一条路由）：

```text
Client 注册 → relay 存入: { device_code, public_key_pem, client_instance_id }
Client 心跳 → relay 续期路由（若密钥已重置，则原子替换 device_code 条目）
密钥撤销   → Client 通知 relay 删除对应 device_code 条目
```

### 3. Client 是唯一的业务决策点

所有需要"判断"的逻辑都在 Client 端：

- 密钥生命周期管理（创建、撤销、重置）
- grant 签发（auth\_method、grant\_mode 等策略）
- 命令 policy 校验
- caller 信息记录与审计
- 连接历史与监控

### 4. 密钥管理集中在被控端 WebUI（单密钥模型）

**每个 Client 实例同一时刻只能有一个 active SSH 密钥**。重新生成密钥后，旧密钥立即失效，Relay 路由表中该 Client 的旧 device\_code 被替换。这意味着：

- Relay 路由表中每个 `client_instance_id` 至多对应一个 `device_code`
- 密钥重置 = 生成新密钥对 + 旧密钥自动 revoke + Relay 路由原子替换
- 不存在"管理多个密钥"的场景，WebUI 只需展示当前密钥状态

SSH 密钥的完整生命周期在 WebUI 的 Remote 控制部分完成：

- **创建**：WebUI 生成 Ed25519 密钥对 + device\_code，用户填写 label；SSH 授权策略固定为永久有效
- **分发**：WebUI 提供一次性复制 device\_code + 私钥的入口
- **监控**：展示使用该密钥连接的 caller 信息
- **重置**：生成新密钥对 + 新 device\_code，旧的立即失效（原子替换，Relay 路由同步更新）
- **撤销**：禁用密钥，通知 relay 删除路由条目，revoke 所有关联 grant

### 5. grant 仍然是统一调用凭证

无论来自授权码还是 SSH，最终都落到统一的 `RemoteInvokeGrant`：

- `client_instance_id`
- `caller_fingerprint`（SSH 模式下 = `ssh_key_fingerprint`）
- `auth_method`
- `grant_mode`
- `remaining_calls / expires_at`

其中 SSH 模式固定签发 `grant_mode=permanent`、`remaining_calls=NULL`、`expires_at=NULL`；授权只会在 SSH key 被重置/撤销后失效。

## 整体架构

```text
┌─────────────────────────────────────────────────────────────────────┐
│                         Caller（agent/CI/人工）                       │
│  持有：Bifrost 密钥文件（内含 device_code + Ed25519 私钥）              │
│  1. 解析密钥文件 → 提取 device_code + 私钥                             │
│  2. POST /ssh/connect { device_code, signature, caller_info }       │
└──────────────────────────────┬──────────────────────────────────────┘
                               │
                               ▼
┌─────────────────────────────────────────────────────────────────────┐
│                        Relay（薄路由层）                              │
│  存储：device_code → { public_key_pem, client_instance_id }         │
│                                                                     │
│  3. 查路由表：device_code → public_key + client_instance_id          │
│  4. 用 public_key 验证 signature                                    │
│  5. 验证通过 → 转发 connect 请求到 Client（通过 SSE/WebSocket）       │
│     验证失败 → 直接拒绝                                              │
└──────────────────────────────┬──────────────────────────────────────┘
                               │
                               ▼
┌─────────────────────────────────────────────────────────────────────┐
│                      Client（被控端，业务决策点）                      │
│  存储：ssh_keys 表（完整密钥信息 + 策略配置）                          │
│        grants 表、calls 表等                                        │
│                                                                     │
│  6. 收到 connect 请求 → 查找 ssh_key 是否 active                     │
│  7. 根据密钥配置签发 grant（mode/max_calls/ttl）               │
│  8. 返回 grant → relay 透传回 caller                                │
│  9. 记录 caller 信息（hostname/IP/platform）用于监控                  │
└─────────────────────────────────────────────────────────────────────┘

后续调用链路（与现有一致）：
  caller → openCall(grant_id) → relay → client executor → 返回结果
```

## 数据模型

### Relay 侧：路由表（内存 + Redis）

Relay 仅维护一张路由映射，无需持久化数据库。**每个** **`client_instance_id`** **至多一条路由**（单密钥模型）：

| 字段                   | 类型           | 说明                        |
| -------------------- | ------------ | ------------------------- |
| `device_code`        | String (Key) | 设备码，`BF-XXXXXXXXXXXXXXXX` |
| `public_key_pem`     | String       | Ed25519 公钥（PEM 格式）        |
| `client_instance_id` | String       | 目标 Client                 |

存储方式：

- Redis Hash: `ri:ssh_route:{device_code}` → `{ public_key_pem, client_instance_id }`
- 由 Client 注册/心跳时写入，密钥撤销时删除
- TTL 跟随 Client 注册的心跳过期（Client 离线则路由自动失效）
- 密钥重置时：Client 同步新 route → Relay 原子替换（删旧 device\_code + 写新 device\_code）

### Client 侧：`remote_invoke_ssh_keys`

Client 本地 SQLite 存储完整的密钥信息和策略配置。**同一时刻至多一条** **`status=active`** **的记录**；创建新密钥时自动将旧密钥 revoke：

| 字段                          | 类型          | 说明                                                            |
| --------------------------- | ----------- | ------------------------------------------------------------- |
| `id`                        | TEXT PK     | UUID                                                          |
| `device_code`               | TEXT UNIQUE | 设备码 `BF-XXXXXXXXXXXXXXXX`                                     |
| `label`                     | TEXT        | 用户自定义标签（如 "CI Agent"）                                         |
| `public_key_pem`            | TEXT        | Ed25519 公钥                                                    |
| `ssh_key_fingerprint`       | TEXT UNIQUE | `SHA256(public_key_der)`                                      |
| `private_key_pem_encrypted` | TEXT        | AES-256-GCM 加密存储的私钥（仅用于 WebUI 复制分发）                           |
| `grant_mode`                | TEXT        | SSH 授权模式，固定为 `permanent`（仅为兼容现有 grant 结构保留字段） |
| `status`                    | TEXT        | `active` / `revoked`                                          |
| `created_at`                | TEXT        | 创建时间                                                          |
| `last_used_at`              | TEXT        | 最后使用时间                                                        |
| `last_caller_info_json`     | TEXT        | 最后连接的 caller 信息                                               |

### Client 侧：扩展 `remote_invoke_grants`

新增字段：

- `auth_method`: `pair_code | ssh_publickey`
- `ssh_key_id`
- `ssh_key_fingerprint`

兼容策略：

- 老 grant 默认 `auth_method = pair_code`

### Client 侧：扩展 `remote_invoke_calls`

新增字段：

- `auth_method`
- `ssh_key_id`
- `ssh_key_fingerprint`
- `caller_info_json`（hostname/IP/platform/user-agent）

<br />

## SSH 鉴权流程

### 前置：Client 注册时同步路由表

```text
Client 启动/注册
  → POST /v4/register { client_instance_id, ..., ssh_device_route: {
      device_code: "BF-A1B2C3D4E5F6A7B8", public_key_pem: "..."
    }}
  → Relay 验证 device_code 的派生关系（从 public_key_pem 独立计算，必须匹配）
  → 验证通过 → 写入路由表:
      ri:ssh_route:BF-A1B2C3D4E5F6A7B8 → { public_key_pem, client_instance_id }
  → 若该 Client 之前有不同的 device_code → 删除旧条目（原子替换）

Client 心跳
  → 携带当前 active 的 ssh_device_route（若无 active 密钥则为 null）
  → Relay 续期路由（或在密钥重置后原子替换）
```

### 连接流程（challenge-sign 两步）

```text
caller
  → 加载 Bifrost 密钥文件 → 解析出 device_code + 私钥
    （若为标准 Ed25519 私钥文件：推导公钥 → 计算 device_code）
  → POST /v4/remote-invoke/ssh/challenge  { device_code }

relay
  → 查路由表确认 device_code 存在
  → 生成 challenge（64 字节随机 nonce，hex 编码 + timestamp），存入 Redis，TTL 120s
  → 返回 { challenge_id, challenge, expires_at }

caller
  → 构造 payload = 按 key 字母序排列的 JSON 字符串：
    `{"challenge":"<nonce_hex>","challenge_id":"<id>","device_code":"<code>","timestamp":<unix_ms>}`
  → 用私钥签名 payload（Ed25519 签名，输入为 payload 的 UTF-8 字节）
  → POST /v4/remote-invoke/ssh/connect {
      device_code,
      challenge_id,
      signature,
      timestamp,
      caller_info: { hostname, username, platform, user_agent }
    }

relay（公钥验签 + 设备 ID 一致性校验）
  → 验证 challenge 有效（未过期、未消费）→ 标记已消费
  → 查路由表: device_code → { public_key_pem, client_instance_id }
  → 用匹配设备的 public_key_pem 解码验证 signature（公钥验签）
  → 签名失败 → 拒绝 ssh_signature_invalid
  → 签名通过 → 从已验证的 payload 中提取 device_code
  → 与请求中声称的 device_code 比对 → 不一致 → 拒绝 device_code_mismatch
  → 一致 → 校验全部通过，将 connect 请求转发给 client_instance_id
      转发内容: { device_code, ssh_key_fingerprint, caller_info, relay_verified: true }

client（收到 relay 转发的 connect，执行二次确认）
  → 二次确认 1: 查找 ssh_keys 表: device_code 匹配且 status=active
  → 未找到 → 拒绝 ssh_key_not_found
  → 已撤销 → 拒绝 ssh_key_revoked
  → 二次确认 2: 验证 relay 转发的 ssh_key_fingerprint 与本地存储的一致
  → 不一致 → 拒绝 ssh_key_fingerprint_mismatch（路由表可能被篡改）
  → 自动签发永久 grant:
      grant_id, auth_method=ssh_publickey, caller_fingerprint=ssh_key_fingerprint,
      grant_mode=permanent, max_calls=NULL, expires_at=NULL
  → 更新 last_used_at, last_caller_info_json
  → 返回 { grant_id, expires_at } → relay 透传回 caller
```

### 与授权码流程的对比

| 维度        | 授权码                                 | SSH 密钥                       |
| --------- | ----------------------------------- | ---------------------------- |
| 授权时机      | 每次配对时 client 审批                     | 密钥创建时已隐式授权                   |
| caller 标识 | `caller_fingerprint`（hostname hash） | `ssh_key_fingerprint`（密钥指纹）  |
| 连接凭证      | pair\_code（动态，几分钟有效）                | device\_code + 私钥签名（永久）      |
| Relay 参与度 | 存储配对状态                              | 公钥验签 + 设备 ID 校验 + 路由（Client 二次确认） |
| 业务决策点     | Relay + Client                      | 仅 Client                     |
| 适用场景      | 人工操作、临时访问                           | CI/CD、长期绑定、agent 沙箱          |
| 撤销方式      | revoke grant                        | 撤销密钥（联动 revoke grant + 删除路由） |

### 异常规则

| 阶段              | 错误码                               | 说明                                              |
| --------------- | --------------------------------- | ----------------------------------------------- |
| Relay 注册/心跳     | `device_code_derivation_mismatch` | device\_code 与 public\_key\_pem 的派生关系不匹配（防路由投毒） |
| Relay challenge | `device_code_not_found`           | 路由表中无此 device\_code                             |
| Relay challenge | `challenge_rate_limited`          | 单 device\_code 超过限流阈值（10 次/分钟）                  |
| Relay connect   | `challenge_expired`               | challenge 过期或已消费                                |
| Relay connect   | `timestamp_out_of_window`         | 请求 timestamp 超出 ±30 秒窗口                         |
| Relay connect   | `ssh_signature_invalid`           | 签名验证失败                                          |
| Relay connect   | `device_code_mismatch`            | 签名 payload 中的 device\_code 与请求声称的不一致（防冒充攻击） |
| Relay connect   | `client_offline`                  | client\_instance\_id 对应的 Client 不在线             |
| Relay connect   | `client_timeout`                  | 等待 Client 二次确认结果超时（30s）                       |
| Client connect  | `ssh_key_not_found`               | 本地无此 device\_code 的密钥记录                         |
| Client connect  | `ssh_key_revoked`                 | 密钥已撤销                                           |
| Client connect  | `ssh_key_fingerprint_mismatch`    | relay 转发的 fingerprint 与本地存储不一致（路由表可能被篡改）    |
| Client connect  | `ssh_key_limit_exceeded`          | 密钥数量超过上限（100）                                   |

## API 设计

### Relay 侧 API（仅 2 个端点）

#### SSH Challenge

`POST /v4/remote-invoke/ssh/challenge`

请求：

- `device_code`

响应：

- `challenge_id`
- `challenge`（随机 nonce）
- `expires_at`

错误：

- `device_code_not_found`

#### SSH Connect

`POST /v4/remote-invoke/ssh/connect`

请求：

- `device_code`
- `challenge_id`
- `signature`（base64）
- `timestamp`
- `caller_info`（可选）
  - `hostname`
  - `username`
  - `platform`
  - `user_agent`

签名 payload 格式（按 key 字母序排列的 JSON 字符串）：

```json
{"challenge":"<nonce_hex>","challenge_id":"<id>","device_code":"<code>","timestamp":<unix_ms>}
```

签名算法：Ed25519，输入为上述 JSON 字符串的 UTF-8 字节，输出 base64 编码。

响应（成功，由 Client 签发，relay 透传）：

- `{ status: "authorized", grant_id, expires_at }`

响应（失败）：

- `{ status: "rejected", reason: "ssh_signature_invalid" | "challenge_expired" | "timestamp_out_of_window" | "client_offline" | "client_timeout" | "ssh_key_revoked" | ... }`

验证流程（按顺序执行）：

1. 读取并消费 challenge（Redis GETDEL 原子操作）→ 失败则 `challenge_expired`
2. 校验 challenge 未过期（`expires_at > now`）→ 失败则 `challenge_expired`
3. 校验 timestamp 在 ±30 秒窗口内 → 失败则 `timestamp_out_of_window`
4. 查路由表获取 public\_key\_pem → 失败则 `device_code_not_found`
5. 构造 payload + 用匹配设备的 public\_key\_pem 解码验证签名 → 失败则 `ssh_signature_invalid`
6. 从已验证的签名 payload 中提取 device\_code，与请求声称的 device\_code 比对 → 不一致则 `device_code_mismatch`
7. 校验全部通过 → 转发给 Client 做二次确认 → 等待结果（最长 30 秒）→ 超时则 `client_timeout`

### Client 侧 API（WebUI 调用）

#### 密钥管理

单密钥模型下，API 简化为对"当前密钥"的操作，无需列表查询：

- `POST /api/remote-invoke/ssh-key` — 创建密钥对（若已有 active 密钥则自动 revoke 旧密钥）
  - 请求：`{ label, grant_mode }`（`grant_mode` 参数被忽略，SSH 一律签发永久授权；保留该字段仅兼容旧客户端）
  - 响应：`{ id, device_code, label, ssh_key_fingerprint, bifrost_key_file, public_key_pem }`
  - 注意：`bifrost_key_file` 为自包含格式（含 device\_code + 私钥），仅此一次返回；同时触发向 relay 同步新路由条目，原子替换旧 device\_code
- `GET /api/remote-invoke/ssh-key` — 获取当前 active 密钥信息（无 active 密钥则返回 null）
  - 响应：`{ id, device_code, label, ssh_key_fingerprint, status, grant_mode, created_at, last_used_at, last_caller_info_json } | null`
- `GET /api/remote-invoke/ssh-key/private-key` — 再次获取密钥文件（WebUI 弹窗确认后返回）
- `POST /api/remote-invoke/ssh-key/reset` — 重置密钥
  - 生成新密钥对 + 新 device\_code，旧的立即失效
  - 通知 relay 删除旧路由 + 写入新路由（原子替换）
  - 联动 revoke 旧密钥的所有 grant
  - 响应：`{ id, device_code, ssh_key_fingerprint, bifrost_key_file, public_key_pem }`
- `DELETE /api/remote-invoke/ssh-key` — 撤销当前密钥
  - 通知 relay 删除路由条目
  - 联动 revoke 所有关联 grant
- `PATCH /api/remote-invoke/ssh-key` — 更新密钥配置（label、grant\_mode）

### Client 内部 API（Relay 转发调用）

#### SSH Connect 处理

`内部路由：处理 relay 转发的 ssh/connect 请求`

请求（relay 转发）：

- `device_code`
- `ssh_key_fingerprint`（relay 从公钥计算）
- `caller_info`
- `relay_verified: true`（relay 已完成公钥验签 + 设备 ID 一致性校验的标记）

处理逻辑（Client 二次确认）：

1. 查 ssh\_keys 表找到 device\_code 对应的密钥
2. 校验 status=active
3. 二次确认：验证 relay 转发的 ssh\_key\_fingerprint 与本地存储的 fingerprint 一致（防路由表篡改）
4. 根据密钥配置签发 grant
5. 更新密钥使用信息
6. 返回 grant 信息

## Relay 双版本实现设计

本方案需要在两个 Relay 实现上同时落地 SSH 鉴权能力。两个版本共享相同的 API 契约，但存储机制和运行模式不同。

### 共同 API 契约

两个 Relay 版本必须实现完全相同的 HTTP 接口：

| 端点                                | 方法   | 作用                 |
| --------------------------------- | ---- | ------------------ |
| `/v4/remote-invoke/ssh/challenge` | POST | 签发 challenge nonce |
| `/v4/remote-invoke/ssh/connect`   | POST | 验签 + 路由转发到 Client  |

Client 注册/心跳时通过已有端点同步 `ssh_device_route`：

| 端点                   | 变更说明                                     |
| -------------------- | ---------------------------------------- |
| `POST /v4/register`  | body 新增 `ssh_device_route` 可选字段（单对象，非数组） |
| `POST /v4/heartbeat` | body 新增 `ssh_device_route` 可选字段（续期或原子替换） |

***

### 版本 A：bifrost-sync-server（本地测试版）

> 路径：`packages/bifrost-sync-server`
> 用途：本地闭环自动化测试、E2E 测试环境
> 特点：单进程、Redis 后端、与现有 remote-invoke 服务并列

#### 架构定位

bifrost-sync-server 是轻量级本地 Relay，已有完整的 remote-invoke 实现（pair\_code、grant、call 路由、SSE 推送）。SSH 鉴权作为并行路径加入，复用现有 Redis 连接和 SSE 基础设施。

#### 存储设计

SSH 路由表使用 Redis，Key 规范沿用现有 `ri:` 前缀：

```text
ri:ssh_route:{device_code}     → JSON { public_key_pem, client_instance_id }    TTL: 跟随心跳 (600s)
ri:ssh_challenge:{challenge_id} → JSON { device_code, challenge, expires_at }    TTL: 120s
```

#### 新增文件

```text
packages/bifrost-sync-server/src/
  remote-invoke/
    ssh-auth.ts         ← SSH 鉴权服务（challenge 签发、签名验证、路由查找）
  routes/
    remote-invoke.ts    ← 新增 SSH 路由端点（挂载到已有路由文件）
```

#### `ssh-auth.ts` 服务接口设计

```typescript
export class SshAuthService {
  constructor(private redis: Redis) {}

  // Client 注册时写入 SSH 路由（须验证 device_code 派生关系）
  async syncSshRoutes(clientInstanceId: string, routes: SshDeviceRoute[]): Promise<void>

  // Client 心跳时刷新路由 TTL（须验证 device_code 派生关系）
  async refreshSshRoutes(clientInstanceId: string, routes: SshDeviceRoute[]): Promise<void>

  // Client 撤销密钥时删除路由
  async removeSshRoute(deviceCode: string): Promise<void>

  // Caller 请求 challenge
  async issueChallenge(deviceCode: string): Promise<ChallengeResponse>

  // Caller 提交签名，验证后转发给 Client
  async verifyAndConnect(body: SshConnectRequest): Promise<ConnectResult>
}

interface SshDeviceRoute {
  device_code: string;
  public_key_pem: string;
}

interface ChallengeResponse {
  challenge_id: string;
  challenge: string;      // 64 字节随机 nonce (hex, 128 字符)
  expires_at: number;     // Unix timestamp ms
}

interface SshConnectRequest {
  device_code: string;
  challenge_id: string;
  signature: string;      // base64
  timestamp: number;
  caller_info?: {
    hostname?: string;
    username?: string;
    platform?: string;
    user_agent?: string;
  };
}
```

#### 路由注册（在 `routes/remote-invoke.ts` 中追加）

```typescript
// SSH challenge
router.post('/v4/remote-invoke/ssh/challenge', async (req, res) => { ... });

// SSH connect (verify + forward to client via SSE)
router.post('/v4/remote-invoke/ssh/connect', async (req, res) => { ... });
```

#### Client → Relay 路由同步

复用已有的 `registerClient` 和 `clientHeartbeat` 流程，在 body 中新增 `ssh_device_route` 可选字段（单对象，非数组）：

```text
POST /v4/remote-invoke/register
  body: { ..., ssh_device_route: { device_code, public_key_pem } | null }
  → 调用 sshAuthService.syncSshRoute(clientInstanceId, route)

POST /v4/remote-invoke/heartbeat
  body: { ..., ssh_device_route: { device_code, public_key_pem } | null }
  → 调用 sshAuthService.refreshSshRoute(clientInstanceId, route)
```

#### 转发机制

验签通过后，通过已有的 `pushToClient()` SSE 机制将 connect 请求推送给 Client：

```text
sshAuthService.verifyAndConnect(body)
  → 验签通过
  → pushToClient(redis, clientInstanceId, 'ssh_connect', {
      device_code,
      ssh_key_fingerprint,   // 从 public_key 计算
      caller_info,
      relay_verified: true   // Relay 已完成公钥验签 + 设备 ID 一致性校验
    })
  → 等待 Client 通过 SSE 回传 grant 结果（或通过 Redis 队列回传）
```

Client 回传 grant 结果的方式：**复用已有的 caller stream 机制**，与 `openCall` → `pushToCallerStream` 完全一致。

```text
复用 caller stream 流程：
1. Caller POST /ssh/connect → Relay 验签通过，生成 connect_id，立即返回 { connect_id }
2. Caller 打开 SSE 订阅 /caller-events?call_id={connect_id}
   → registerCallerEventStream(connect_id, res)   // 复用已有函数
3. Relay pushToClient(clientInstanceId, 'ssh_connect', { connect_id, device_code, ... })
4. Client 决策 grant → POST /ssh/connect-result { connect_id, status, grant_id, ... }
5. Relay 收到 connect-result → pushToCallerStream(connect_id, 'ssh_connect_result', { grant_id, ... })
   // 复用已有函数，支持本地 SSE 直推 + Redis 队列跨实例投递
6. Caller 通过 SSE 收到结果，后续使用 grant_id 进入 openCall 调用链
```

优势：零新增基础设施，`registerCallerEventStream` / `pushToCallerStream` / Redis 队列跨实例投递全部复用，keepalive 和断线清理逻辑也自动继承。

#### 测试适配

E2E 测试环境直接启动 bifrost-sync-server，通过 CLI → sync-server → Client 完成完整链路验证：

```bash
# 启动本地 sync-server（E2E 模式）
cd packages/bifrost-sync-server && pnpm start --port 3200

# 启动 Client（连接本地 sync-server）
BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 \
  --relay-url http://localhost:3200 --unsafe-ssl --no-system-proxy

# CLI 使用 SSH 密钥连接
bifrost remote connect --ssh-key ./test-key.bifrost --relay-url http://localhost:3200
```

***

### 版本 B：bifrost-server-v4（生产部署版）

> 路径：`bifrost-server-v4`
> 用途：真实环境生产部署
> 特点：多实例、Redis 集群、`@gulu/application-http` 框架、跨实例事件投递

#### 架构定位

bifrost-server-v4 是生产级 Relay 服务，运行在字节内部基础设施上。已有 remote-invoke 相关代码（`app/service/remoteInvoke.ts`、`app/routes/remoteInvoke.ts`、`app/helper/remoteInvokeSse.ts`），SSH 鉴权在此基础上扩展。

为了和本地 `bifrost-sync-server` 保持行为一致，生产版还必须满足三条兼容要求：

- `ssh/connect-result(status=approved)` 必须把 SSH grant 持久化到 relay 存储，而不是只透传结果给 caller
- 持久化后的 SSH grant 必须写入 `grant_idx(client_instance_id)`，这样 `grants/reusable` 和 `calls/open` 才能命中
- 当 Client 删除或轮换 SSH route 时，Relay 必须同步撤销该 Client 旧的 `ssh_publickey` grants，避免旧密钥残留复用

#### 存储设计

与本地版相同的 Redis Key 规范，但在生产环境中需考虑：

```text
ri:ssh_route:{device_code}      → JSON { public_key_pem, client_instance_id }    TTL: 600s（心跳续期）
ri:ssh_challenge:{challenge_id}  → JSON { device_code, challenge, expires_at }    TTL: 120s
ri:mq:caller:{connect_id}       → 复用已有 caller stream Redis 队列（跨实例投递 connect 结果）
```

生产环境额外考虑：

- Redis 集群部署：所有 SSH 相关 Key 使用 `{device_code}` 作为 hash tag 确保同 slot
- 多实例一致性：challenge 存储在 Redis 中（非内存），任意实例均可验证
- 单密钥模型：每个 `client_instance_id` 至多一条路由，路由总量 = Client 在线数量，无容量压力

#### 新增/修改文件

```text
bifrost-server-v4/app/
  service/
    remoteInvoke.ts     ← 在现有 Service 中新增 SSH 相关方法
  routes/
    remoteInvoke.ts     ← 新增 SSH 路由端点
  helper/
    remoteInvokeSse.ts  ← 复用现有 SSE 推送能力
```

#### Service 扩展（在 `RemoteInvokeService` 中新增方法）

```typescript
// === SSH 路由管理 ===

async syncSshDeviceRoute(clientInstanceId: string, route: {
  device_code: string;
  public_key_pem: string;
} | null): Promise<void> {
  if (route) {
    // 0. 验证 device_code 的派生关系（防路由投毒，见安全要求 T2）
    // 1. 查询该 clientInstanceId 当前绑定的旧 device_code（若有）
    // 2. 若旧 device_code !== 新 device_code → 删除旧路由条目
    // 3. 写入新路由条目（SETEX）
  } else {
    // route 为 null：清除该 Client 的路由（密钥已撤销）
    // 1. 查询当前绑定的 device_code → 删除
  }
}

async removeSshDeviceRoute(clientInstanceId: string): Promise<void> {
  // 查询当前绑定的 device_code → 删除路由条目
}

// === SSH Challenge ===

async issueSshChallenge(deviceCode: string): Promise<{
  challenge_id: string;
  challenge: string;
  expires_at: number;
}> {
  // 1. 检查路由表中 device_code 存在
  // 2. 生成随机 challenge_id + nonce
  // 3. 存入 Redis, TTL 120s
  // 4. 返回
}

// === SSH Connect ===

async verifySshConnect(body: {
  device_code: string;
  challenge_id: string;
  signature: string;
  timestamp: number;
  caller_info?: object;
}): Promise<{ connect_id: string }> {
  // 1. 读取并消费 challenge（GETDEL 原子操作，防重放）
  // 2. 验证 challenge 未过期
  // 3. 校验 timestamp 在 ±30 秒窗口内（防延迟提交）
  // 4. 查路由表获取 public_key_pem + client_instance_id
  // 4. 构造 payload = {"challenge":..., "challenge_id":..., "device_code":..., "timestamp":...}（key 字母序 JSON）
  // 5. crypto.verify(null, payload, publicKey, signature)（公钥解码验签）
  // 6. 签名通过 → 从 payload 中提取 device_code，与请求声称的 device_code 比对 → 不一致则 device_code_mismatch
  // 7. 校验全部通过 → 生成 connect_id（crypto.randomBytes(16).toString('hex')，密码学安全）
  // 8. pushToClient(clientInstanceId, 'ssh_connect', { connect_id, device_code, caller_info, relay_verified: true, ... })
  // 9. 立即返回 { connect_id }，caller 通过 caller stream SSE 等待 Client 二次确认结果
}
```

#### 路由注册

```typescript
// 在 app/routes/remoteInvoke.ts 中追加

router.post('/v4/remote-invoke/ssh/challenge', async (ctx) => {
  const { device_code } = ctx.request.body;
  const result = await ctx.service.remoteInvoke.issueSshChallenge(device_code);
  ctx.body = { code: 0, data: result };
});

router.post('/v4/remote-invoke/ssh/connect', async (ctx) => {
  const result = await ctx.service.remoteInvoke.verifySshConnect(ctx.request.body);
  // 返回 connect_id，caller 随后通过 /caller-events?call_id={connect_id} 订阅结果
  ctx.body = { code: 0, data: result };
});

// Caller 可复用已有的 /v4/remote-invoke/caller-events SSE 端点，以 connect_id 作为 call_id 订阅

// Client 回传 connect 结果（需验证 client_auth_token，复用 caller stream 推送）
router.post('/v4/remote-invoke/ssh/connect-result', async (ctx) => {
  // 鉴权：验证 client_auth_token → 提取 client_instance_id
  // 安全校验：验证 connect_id 对应的路由目标 client_instance_id 与请求者一致
  const { connect_id, status, grant_id, expires_at, reason } = ctx.request.body;
  // 通过 caller stream 推送结果给等待中的 caller SSE
  await pushToCallerStream(ctx.redis, connect_id, 'ssh_connect_result', { status, grant_id, expires_at, reason });
  ctx.body = { code: 0 };
});
```

#### 跨实例事件投递

生产环境 Relay 可能多实例部署，connect 请求和 Client SSE 不一定在同一实例。**完全复用已有的 Redis List + Lua DRAIN 跨实例投递机制**，无需新增任何基础设施：

```text
/ssh/connect 请求到达 Instance A
  → pushToClient() 将 ssh_connect 事件写入 Redis Queue: ri:mq:client:{client_instance_id}
  → Client 的 SSE 连接在 Instance B 上
  → Instance B 的 poller 从 Redis Queue 读取事件 → 通过 SSE 推送给 Client
  → Client 处理后 POST /v4/remote-invoke/ssh/connect-result 到任意 Instance C
  → Instance C 调用 pushToCallerStream(connect_id, 'ssh_connect_result', result)
     - 如果 caller SSE 在 Instance C 本地 → 直接写入 SSE
     - 如果 caller SSE 在 Instance A → 写入 Redis Queue ri:mq:caller:{connect_id}
       → Instance A 的 poller 读取后推送给 caller
```

#### 与现有注册/心跳的集成

在现有的 `registerClient` 和 `clientHeartbeat` 方法中追加 SSH 路由同步：

```typescript
// registerClient 中追加：
if (body.ssh_device_route !== undefined) {
  await this.syncSshDeviceRoute(cid, body.ssh_device_route);
}

// clientHeartbeat 中追加：
if (body.ssh_device_route !== undefined) {
  await this.syncSshDeviceRoute(clientInstanceId, body.ssh_device_route);
}
```

#### 生产环境特殊考量

| 维度   | 设计决策                                               |
| ---- | -------------------------------------------------- |
| 高可用  | SSH 路由表、challenge 均存 Redis，任意实例可处理                 |
| 超时控制 | connect 请求最多等待 Client 回传 30s，超时返回 `client_timeout` |
| 限流   | 单 device\_code 每分钟最多 10 次 challenge 请求（防暴力试探）      |
| 监控   | 上报 SSH connect 成功/失败指标到 metrics（BAM）               |
| 审计   | 记录 SSH connect 事件（caller IP、device\_code、结果）到日志    |
| 清理   | Client 离线（心跳过期）→ 路由自动过期（Redis TTL）                 |

***

### 两版本功能对照表

| 功能           | sync-server（本地）       | server-v4（生产）         |
| ------------ | --------------------- | --------------------- |
| SSH 路由存储     | Redis（单实例）            | Redis（集群/多实例）         |
| Challenge 存储 | Redis（TTL 120s）       | Redis（TTL 120s）       |
| 转发机制         | 内存 SSE + Redis Queue  | 纯 Redis Queue（跨实例）    |
| Client 结果回传  | HTTP POST + Redis Key | HTTP POST + Redis Key |
| 连接超时         | 30s                   | 30s                   |
| 限流           | 无（测试环境）               | 有（每分钟/device\_code）   |
| 监控           | console.log           | BAM metrics + 审计日志    |
| 多实例          | 否（单进程）                | 是（N 实例无状态）            |
| 部署方式         | `pnpm start`          | 容器化部署                 |
| 测试覆盖         | E2E 自动化测试主阵地          | 生产验证                  |

***

### Client（Rust 侧）对两版本 Relay 的适配

Client 不需要区分两个版本，只需：

1. **注册/心跳时**：在已有的 body 中添加 `ssh_device_route` 字段（当前 active 密钥的 device\_code + public\_key\_pem，无密钥则为 null）
2. **处理 SSH connect 事件**：监听 SSE `ssh_connect` event，查本地 ssh\_keys 表，签发 grant
3. **回传结果**：POST `{relay_url}/v4/remote-invoke/ssh/connect-result` 将永久 grant 信息回传；relay 必须按 SSH 语义落库为 `grant_mode=permanent`、`expires_at=NULL`

```rust
struct RegisterBody {
    // ... 现有字段 ...
    ssh_device_route: Option<SshDeviceRoute>,
}

struct SshDeviceRoute {
    device_code: String,
    public_key_pem: String,
}

// SSE 事件处理扩展
match event_type {
    "ssh_connect" => {
        let req: SshConnectEvent = serde_json::from_value(data)?;
        let result = handle_ssh_connect(&db, &req).await;
        // POST 回传 result 到 relay
        post_ssh_connect_result(&relay_url, &client_auth_token, &result).await;
    }
}
```

## `openCall` 改造

`openCall` 继续保留现有 grant 主校验：

- grant 存在
- `caller_fingerprint` 匹配
- `client_instance_id` 匹配
- grant active / 未过期 / remaining calls 合法

当 `grant.auth_method == ssh_publickey` 时，追加：

1. 对应的 `ssh_key` 必须存在且 status=`active`
2. grant 的 `ssh_key_fingerprint` 必须与请求中的一致

之后进入统一的命令执行链路。

## 安全要求

### 威胁模型与安全分析

#### 🔴 已识别高风险威胁及缓解

**T1: device\_code 碰撞与暴力搜索**

- 威胁：攻击者通过高速生成 Ed25519 密钥对，寻找与目标 device\_code 相同的密钥
- 缓解：device\_code 使用 **8 字节（64 位）** 派生（SHA256 前 8 字节），碰撞搜索空间 \~1.8×10^19，实际不可行
- 对比：旧设计 4 字节（32 位）仅 \~43 亿种可能，Ed25519 密钥生成速度（\~数十万对/秒）下数分钟即可碰撞

**T2: 路由表投毒（恶意 Client 注册他人 device\_code）**

- 威胁：已认证的恶意 Client 在注册时声称拥有他人的 device\_code，关联自己的公钥覆盖合法路由
- 缓解：**Relay 在注册/心跳时必须验证 device\_code 的派生关系** — 从 `public_key_pem` 独立计算 `hex(SHA256(public_key_der)[0..8])`，与声称的 device\_code 比对，不匹配则拒绝
- 实现：注册和心跳路径中均须执行此校验，不可跳过

#### 🟡 已识别中风险威胁及缓解

**T3: connect-result 注入**

- 威胁：攻击者猜测或截获 `connect_id`，抢先注入虚假 grant 结果到 `ri:ssh_result:{connect_id}`
- 缓解措施：
  1. `connect_id` 必须使用密码学安全随机数生成（≥16 字节 hex，如 `crypto.randomBytes(16).toString('hex')`）
  2. `POST /ssh/connect-result` 端点必须验证：请求携带的 `client_auth_token` 对应的 `client_instance_id` 与该 `connect_id` 关联的路由目标一致
  3. `ri:ssh_result:{connect_id}` 使用 Redis `SET ... NX`（仅首次写入生效），防止覆盖

**T4:** **`verified: true`** **信任链**

- 威胁：如果 Client 暴露了接收 SSH connect 请求的 HTTP 端点，攻击者可以伪造 `verified: true` 绕过签名验证
- 缓解：**Client 仅通过 SSE 通道接收** **`ssh_connect`** **事件**，不为此开放独立的 HTTP 端点。SSE 通道是 Client 主动建立的到 Relay 的认证连接，天然具备来源可信性。Client 的 `ssh_connect` 事件处理逻辑必须仅绑定在 SSE 事件分发器中

**T5: timestamp 时间窗口**

- 威胁：攻击者获取 challenge 后延迟提交签名请求（在 challenge TTL 120s 内的不利时机）
- 缓解：Relay 在 `/ssh/connect` 验证时，除 challenge 有效性外，还必须校验 `timestamp` 在 ±30 秒窗口内（与现有注册流程的 `REGISTER_TIMESTAMP_SKEW_MS` 保持一致）
- 异常码：`timestamp_out_of_window`

**T6: 私钥再分发端点**

- 威胁：`GET /api/remote-invoke/ssh-key/private-key` 端点若认证不充分，可被利用获取私钥
- 缓解措施：
  1. 必须重新验证 WebUI 认证凭据（不依赖已有 session，要求重新输入密码或二次认证）
  2. 访问频率限制：单密钥每 24 小时最多获取 3 次
  3. 每次访问记录审计日志（包含操作者、时间、IP）
  4. 考虑替代方案：移除此端点，改为"丢失即重置"模式（更安全但牺牲便利性）

**T7: 路由投毒导致 Caller 被投毒（设备冒充 / 路由劫持）**

- 威胁：攻击者持有合法密钥 A（对应 device\_code\_A），但在 `/ssh/connect` 请求中声称 device\_code\_B，试图冒充设备 B 获取路由到另一个 Client 的访问权限
- **Caller 被投毒后果**：若 Relay 不做 device\_code 一致性校验而直接转发，Caller 会收到错误 Client 返回的伪造 grant 或恶意响应数据——Caller 无法区分真伪，信任链从源头被破坏
- 攻击路径：若 Relay 仅验签但不校验 payload 中的 device\_code，攻击者可将签名绑定到错误的 device\_code 路由上，Relay 无脑转发导致 Caller 被投毒
- 缓解：**Relay 在签名验证通过后，必须从已验证的 payload 中提取 device\_code，与请求声称的 device\_code 做一致性比对**，不一致则返回 `device_code_mismatch`，从源头阻断投毒数据流向 Caller
- 纵深防御：即使 Relay 校验被绕过，Client 二次确认时会验证 ssh\_key\_fingerprint 与本地存储一致，形成双重保护

**T8: Relay 路由表被篡改导致 Caller 接收到非预期 Client 的响应（fingerprint 不一致）**

- 威胁：Relay 路由表被攻破或缓存不一致，导致请求被转发给错误的 Client，**Caller 最终接收到非目标设备返回的数据，被投毒而不自知**
- 攻击路径：攻击者篡改路由表中 device\_code\_X 的 client\_instance\_id 指向恶意 Client，Relay 验签通过后将请求转发给恶意 Client，恶意 Client 返回伪造数据给 Caller
- 缓解：**Client 在签发 grant 前，必须验证 relay 转发的 ssh\_key\_fingerprint 与本地 ssh\_keys 表中该 device\_code 对应的 fingerprint 一致**，不一致则返回 `ssh_key_fingerprint_mismatch`
- 意义：这是 **Caller 防投毒的最后一道防线**——Relay 验签 + 设备 ID 校验是第一道防线阻断大部分投毒攻击，Client 二次确认是第二道防线拦截路由表被深度篡改的场景，两者独立互不依赖

### SSH 密钥安全要求

- Bifrost 密钥文件（含 device\_code + 私钥）在 WebUI 创建时展示一次，后续可通过 `/private-key` 端点再次获取（需 WebUI 弹窗确认）
- 私钥在本地 SQLite 存储时使用 **AES-256-GCM** 加密，加密密钥（`ssh_encryption_key`）存储在启动数据目录的 `admin/auth.db` 中（与 `jwt_secret` 同级），首次创建 SSH 密钥时自动生成
- 密钥撤销必须原子性同步：revoke 所有关联 grant + 通知 relay 删除路由，不允许遗留
- challenge nonce：64 字节随机数，单次消费（GETDEL 原子操作），存储在 Redis（TTL 120s），防重放
- Relay 不持有任何私钥，仅持有公钥用于验签
- 单个 Client 同一时刻至多 **1** 个 active SSH 密钥（单密钥模型），路由表无膨胀风险

### Relay 安全边界

> **核心安全目标：保护 Caller 不被投毒。** Relay 的所有验证机制（验签、设备 ID 校验）都是为了在转发前确保数据来源合法且路由目标一致，防止路由表被篡改后 Caller 接收到恶意数据。

- Relay 不做业务决策，无法被利用绕过 Client 的 policy
- Relay 路由表仅包含公钥（非敏感信息）+ client\_instance\_id
- **Relay 必须验证 device\_code 的派生关系**（见 T2），防止路由投毒
- **Relay 必须在公钥验签后校验 payload 中 device\_code 与请求声称值的一致性**（见 T7），防止设备冒充导致 Caller 被投毒
- **Relay 校验通过后转发给 Client，Client 执行二次确认**（见 T8）：独立验证 fingerprint 一致性，作为 Caller 防投毒的最后一道防线
- 即使 Relay 被攻破，攻击者仍无法伪造签名（没有私钥），且 Client 二次确认会拦截不一致的 fingerprint
- 路由条目跟随 Client 心跳自动过期，Client 离线后路由自动清除
- 生产环境单 device\_code 限流 10 次 challenge/分钟（见 T1 辅助缓解）

### Bifrost 密钥文件安全

- 密钥文件中私钥为 base64 明文（无密码保护），**文件本身即凭证**
- 分发安全要求：
  - 文件权限必须设为 `0600`（仅所有者可读写）
  - 必须通过加密通道分发（HTTPS、加密聊天、密钥管理服务）
  - 禁止通过明文邮件、公共聊天频道、未加密的文件共享传输
  - CI/CD 场景建议使用环境变量注入（`--ssh-key env:BIFROST_SSH_KEY`），避免写入磁盘
- CLI 从文件读取后应检查文件权限，权限过宽时输出警告

## CLI 与 UI 改造

### CLI

保留：

- `bifrost remote connect <pair-code>`

新增：

- `bifrost remote connect --ssh-key <path_or_env>`

SSH 连接支持多种密钥来源：

- `--ssh-key ~/.bifrost/remote.key` — 从 Bifrost 密钥文件读取（内含 device\_code + 私钥）
- `--ssh-key ~/.ssh/bifrost_ed25519` — 从标准 Ed25519 私钥文件读取（自动推导公钥 → 计算 device\_code）
- `--ssh-key env:BIFROST_SSH_KEY` — 从环境变量读取（适合 CI/CD，内容为 Bifrost 密钥文件格式）
- `--ssh-key -` — 从 stdin 读取

device\_code 解析策略：

1. Bifrost 格式文件 → 直接从 `Device-Code:` header 解析
2. 标准 Ed25519 私钥 → 推导公钥 → `BF-` + `hex(SHA256(public_key_der)[0..8])`
3. 可选 `--device-code BF-XXXXXXXXXXXXXXXX` 覆盖（仅用于调试/特殊场景）

本地连接文件新增：

- `auth_method`
- `ssh_key_fingerprint`
- `ssh_key_source`（文件路径 / env var 名）
- `device_code`（从密钥文件解析或派生）

### Admin UI（WebUI Remote 控制部分）

Remote Invoke 页面新增 **SSH 密钥管理** 区域（单密钥模型，无列表视图）：

#### 无密钥状态

当无 active 密钥时，展示空状态 + 「创建 SSH 密钥」按钮。

#### 当前密钥视图（有 active 密钥时）

| 信息项         | 内容                                         |
| ----------- | ------------------------------------------ |
| Label       | 用户自定义标签                                    |
| Device Code | `BF-XXXXXXXXXXXXXXXX`                      |
| Fingerprint | `SHA256:xxxx...`                           |
| Status      | `active`                                   |
| SSH Access  | `permanent`（直到 SSH key 被重置或撤销） |
| Last Used   | 最后使用时间                                     |
| Last Caller | hostname / IP / platform                   |
| Actions     | 复制连接信息 · 重置 · 撤销 · 编辑                      |

#### 创建密钥对话框

- Label（必填）
- 固定说明：SSH grants are always permanent，时间型 Grant Mode 不适用于 SSH
- 若已有 active 密钥，弹出二次确认：「创建新密钥将自动撤销当前密钥及其所有关联的 grant，是否继续？」
- 创建后弹出连接信息展示框：
  - Device Code: `BF-A1B2C3D4E5F6A7B8`（展示，供参考）
  - Bifrost 密钥文件内容（带一键复制按钮，格式为 `-----BEGIN BIFROST KEY-----` 自包含格式）
  - CLI 使用示例：`bifrost remote connect --ssh-key <path>`
- 提示：「此密钥文件仅展示一次，请立即复制并安全保存」

#### 密钥操作区

- 基本信息（label、device\_code、fingerprint、状态、创建时间）
- 授权说明（永久有效，直到密钥被重置或撤销）
- 连接历史（最近 N 次连接的 caller 信息）
- 操作按钮：
  - 「再次获取密钥文件」— 弹出确认对话框后展示 Bifrost 密钥文件
  - 「重置密钥」— 生成新密钥对 + 新 device\_code，旧密钥自动 revoke，展示新密钥文件
  - 「撤销密钥」— 确认后禁用，联动 revoke grant + 删除 relay 路由，回到无密钥状态

## 推荐落地顺序

### Phase 1：SSH 密钥管理 + WebUI + 路由同步

- Client 端新增 `remote_invoke_ssh_keys` 表（单密钥约束：至多一条 active 记录）
- WebUI 密钥管理（创建、当前密钥视图、复制 Bifrost 密钥文件、撤销、重置）
- Client 注册/心跳时同步 ssh\_device\_route 到 Relay（单对象，非数组）
- `ssh_device_route` 同步必须保持三态语义：字段缺失表示 Client 本地 key store 读取失败，Relay 不应修改现有 route；字段值为 `null` 表示 Client 已无 active SSH key，Relay 必须删除旧 device\_code route 并 revoke 对应 SSH grant；对象值表示发布或更新 active route。
- **bifrost-sync-server**：实现 `SshAuthService` + 路由存储
- **bifrost-server-v4**：在 `RemoteInvokeService` 中扩展路由同步方法

### Phase 2：SSH challenge + 自动授权

- **bifrost-sync-server**：实现 `/ssh/challenge` + `/ssh/connect` 端点
- **bifrost-server-v4**：实现 `/ssh/challenge` + `/ssh/connect` 端点 + `/ssh/connect-result` 回调
- Client 实现 SSE `ssh_connect` 事件处理 + grant 自动签发 + 结果回传
- Caller CLI 实现 `--ssh-key` 参数（Bifrost 密钥文件解析 + device\_code 自动提取）
- E2E 测试（基于 bifrost-sync-server 本地环境）

### Phase 3：连接监控 + 生产部署

- caller 连接信息实时推送到 WebUI
- 连接历史记录
- **bifrost-server-v4** 生产环境限流、监控、审计日志接入
- 生产环境全链路验证

## MVP 范围

最小可行版本只包含：

1. WebUI SSH 密钥管理（创建、列表、复制 Bifrost 密钥文件、撤销）
2. Client 注册/心跳同步路由表到 Relay
3. **bifrost-sync-server**：SSH challenge/connect 完整实现（本地测试主阵地）
4. **bifrost-server-v4**：SSH challenge/connect 完整实现（生产部署）
5. Client 处理 SSH connect 请求并签发 grant + 结果回传
6. `openCall` 的 SSH key 状态校验
7. caller 信息展示
8. 密钥撤销联动（revoke grant + 删除 relay 路由）
9. CLI `--ssh-key` 支持（自动解析 device\_code）

以下延后：

- 密钥重置（Phase 3）
- 连接历史记录
- key rotation
- UI 完整编辑器
- 生产环境限流/监控（Phase 3）

## 测试方案

### 单元测试

- SSH 密钥创建（Ed25519 生成、fingerprint 计算、device\_code 确定性派生 8 字节、Bifrost 密钥文件格式序列化/反序列化、私钥加密存储）
- device\_code 派生验证（正确公钥通过、篡改 device\_code 拒绝、篡改 public\_key 拒绝）
- challenge 签名验证（正确签名通过、伪造签名拒绝、过期 challenge 拒绝、timestamp 超窗口拒绝）
- connect\_id 密码学随机性验证（≥16 字节、不可预测）
- connect-result 写入（首次写入成功、重复写入被 NX 拒绝）
- Relay 路由表 CRUD（写入、查找、删除、心跳更新、device\_code 派生校验失败拒绝）
- Relay 路由同步三态语义（`ssh_device_route` 缺失不清理 route，`null` 清理旧 route，object 发布 route）
- Client connect 处理（密钥查找、状态校验、grant 签发、密钥数量上限校验）
- 密钥撤销联动 grant revoke + 路由删除
- `openCall` 的 SSH key 状态校验

### E2E 测试

基于 bifrost-sync-server（本地版）执行完整自动化测试：

- 授权码链路不回归
- WebUI 创建 SSH 密钥 → Client 注册同步路由到 sync-server → CLI 使用 Bifrost 密钥文件连接 → grant 签发成功 → 远程命令执行成功
- 撤销密钥 → sync-server 路由被删除 → 使用该密钥连接被拒绝
- 密钥重置 → 旧 device\_code 连接被拒绝，新 device\_code 连接成功
- challenge 过期验证 → 等待 120s 后使用旧 challenge → 应被拒绝
- 伪造签名验证 → 使用错误私钥签名 → 应被拒绝
- Client 离线验证 → 路由 TTL 过期后连接 → 应返回 `client_offline`
- 路由投毒防护 → 恶意 Client 注册篡改的 device\_code → 应返回 `device_code_derivation_mismatch`
- timestamp 超窗口 → 使用超过 ±30s 的 timestamp → 应返回 `timestamp_out_of_window`
- 密钥数量上限 → 创建超过 100 个密钥 → 应返回 `ssh_key_limit_exceeded`

### bifrost-server-v4 集成测试

生产版 Relay 的额外测试覆盖（可在 PPE 环境执行）：

- 多实例场景：challenge 在 Instance A 签发，connect 在 Instance B 验证
- 跨实例转发：connect 请求在 Instance A，Client SSE 在 Instance B
- 限流验证：单 device\_code 超过 10 次/分钟 challenge 请求 → 应被限流
- Redis 故障降级：Redis 不可用时的错误处理（不 panic，返回 503）

### Human Tests

更新 `human_tests/remote-invoke.md`，新增 SSH 授权真实场景用例，覆盖：

- WebUI 创建密钥、复制 Bifrost 密钥文件
- 创建时即使传入 `grant_mode=30m`，返回与实际签发结果也必须归一化为 `permanent`
- CLI 使用 Bifrost 密钥文件连接（device\_code 自动解析）
- CLI 使用标准 Ed25519 私钥文件连接（device\_code 自动计算）
- CLI 使用环境变量传递密钥文件连接（模拟 CI/CD 场景）
- 密钥撤销后连接被拒
- 密钥重置后旧 device\_code 失效、新 device\_code 生效
- WebUI 展示 caller 连接信息

## 校验要求

功能实现阶段必须执行：

- Remote Invoke 相关单元测试
- 对应 E2E 套件
- `cargo test --workspace --all-features`
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `bash scripts/ci/local-ci.sh` 按影响面选择执行

## 2026-04-21 CLI 落地补充

- `bifrost remote connect` 已补齐 `--ssh-key` 参数，支持从导出的 Bifrost key 文件、`env:NAME`、stdin 与 PKCS#8 Ed25519 私钥发起 SSH connect
- caller 侧在 SSH connect 成功后会把连接保存进 `{BIFROST_DATA_DIR}/remote-connections.json`，并复用保存的 `caller_fingerprint` 执行后续 `remote status/search/traffic`
- relay 的 `ssh_connect_result` SSE 事件必须携带 `client_instance_id`，否则 caller 无法完成本地连接落盘与后续命令复用
- `bifrost-server-v4` 的 `ssh_connect` 挂起态也必须持久化 `caller_info`，否则 `connect-result` 落 SSH grant 时会丢失 `caller_display_name`，导致 WebUI / grant 列表无法展示调用方信息

## 2026-05-06 CI 稳定性补充

- SSH key reset 会轮换 `device_code` 与 route 信息，并通过 `trigger_ssh_route_refresh()` 让 remote invoke worker 断开 SSE 后重新注册。CI 验收必须等待 reset 后的 worker 真实回到 `Connected`，再验证新 `device_code` 的 challenge。
- macOS arm64 shell shard 在高并发下可能让 reset 后重连超过原来的 60s 窗口；`test_remote_invoke_ssh_e2e.sh` 的 reset 后重连等待放宽为 `BIFROST_E2E_REMOTE_INVOKE_RESET_RECONNECT_TIMEOUT`，默认 180s。
- 如果 reset 后重连仍超时，E2E 必须输出最后的 remote invoke status、relay log tail 与 Bifrost log tail，避免 CI 只给出 `unknown failure` 或单行等待失败。
