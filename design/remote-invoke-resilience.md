# Remote Invoke 授权韧性改进

## 背景

Remote Invoke 通过 Relay 中转 Caller 与 Target 之间的授权、命令与回帧流量。早期实现把「grant 的真相」寄存在 Relay，Target 只是被动接收 SSE 事件；一旦网络抖动、Relay 短暂不可达或 Target 进程重启，就会出现下列不一致：

1. SSE 断线期间丢失的 `grant_revoked` 事件在 Target 侧无补偿路径，形成「Relay 已删除但 Target 仍认为有效」的僵尸 grant。
2. `local_grants` 只放在内存的 `Arc<RwLock<HashMap<..>>>`；Target 进程重启后所有本地 grant 状态归零，需要靠 Relay 侧再次广播恢复。
3. Caller CLI 在 `find_reusable_grant` 返回空时直接 `exit(1)`，`remote-connections.json` 中的条目残留成脏数据，用户不得不手工清理。
4. Recent Calls 参数预览 E2E 依赖 `pick_free_port` 挑本地端口，`http_echo_server.py` 的 fallback 机制导致脚本在实际端口就绪后仍继续轮询原端口，造成 CI 假失败。
5. Bifrost 启动即拉起 remote-invoke worker，但 `sync-state.json` 还没登录时 worker 会周期性 `ERROR failed to register with relay`，把「等待用户登录」和真实故障混在一起。
6. PPE 全链路复测中出现过 `post_call_frame` reqwest 发送层瞬断，服务端已 200 但客户端未收到 ACK，导致 call 直接失败。
7. `bifrost remote conn up --ssh-key` 复用 SSH 长期 key 时每次连接都签一份 session grant，Grants 列表随时间线性增长。

本方案在不改变授权协议的前提下，把「grant 真相 + 时间线 + 清理边界」下沉到 Target 本地，Relay 只作为增量事件源，让 Caller / Target / WebUI 三端都能自愈。

## 用户目标验证清单

### 必须实现

- SSE 重连后 Target 以 Relay 返回的 active grants 集合为权威，剔除本地过期 grant。
- `GrantInfoStore` 落盘 `remote_invoke_grant_info.json`，Target 重启后能从磁盘还原本地 grant 元数据。
- Grants 列表展示两个可诊断时间戳：`first_connected_at`、`last_command_at`。
- Admin API 与 CLI `bifrost setting grant list` 都返回并展示这两个时间戳，未执行命令时展示 `-`。
- Caller CLI 在 relay 返回 stale grant 时自动清理 `remote-connections.json` 并给出下一步修复建议。
- Recent Calls E2E 解析 `http_echo_server.py` 日志中的 `Starting HTTP Echo Server on <host>:<actual_port>...`，用实际端口做健康检查与流量注入。
- 未登录 sync session 时 worker 只打一次 `INFO remote invoke relay registration waiting for sync session token`，退避后自动重试。
- Target 侧 `post_call_frame` / `post_call_stream_frame` 遇到 send error 时按 `150ms/500ms/1000ms` 短重试。
- `list_grants_and_cleanup()` 按 `max(first_authorized_at, last_used_at, last_command_at)` 计算活动时间，48 小时以上无活动的 stale grant 被清理，正在执行 active call 的 grant 不被误删。

### 必须不破坏

- 已发布的 grant 协议（`grant_id` / `grant_mode` / `caller_fingerprint` 语义）不改。
- 主链路 `openCall → executor → relay 回帧` 的加密与 fingerprint 校验行为不改。
- SSH key 一级默认 file access policy 不因 stale grant cleanup 被删。
- Grants API 保留 `first_authorized_at` / `last_used_at` 兼容字段。

### 必须真实验证

- CLI + Admin API 真实展示 `first_connected_at` / `last_command_at`，且只有 executor 真的跑命令时 `last_command_at` 才刷新。
- `test_remote_invoke_recent_calls_args_preview_e2e.sh` 在预占请求端口场景下能自动 fallback，脚本全绿。
- 空 `BIFROST_DATA_DIR` 启动只出现 1 条 INFO 等待日志，无重复 ERROR。
- 使用 SSH key 反复连接 100 次，48 小时后 stale session grant 被清理，正在跑的 call 不受影响。

## 产品语义

### 双时间戳的含义

- `first_connected_at`：第一次在 Target 侧建立授权/连接的时间。pair-code grant 取审批落地时间，SSH grant 取 `ssh/connect` 成功时间。
- `last_command_at`：仅当 `call_open` 通过本地 grant 校验并真正把命令送入 executor 时更新；SSE 对账、grant 列表刷新、SSH 连接刷新都不会触碰它。

WebUI Settings → Remote Invoke → Grants 行内展示 `connected <time>` 与 `last command <time>`；CLI `bifrost setting grant list` 展示 `first connected` 与 `last command`。

### Stale grant 清理策略

- 每次 `list_grants_and_cleanup()` 和周期性 cleanup 计算最近活动时间：`max(first_authorized_at, last_used_at, last_command_at)`。
- 超过 48h 无活动 → 删除 grant 与其关联的 crypto / policy / grant_info 记录，同步落盘。
- 正在执行 active call 的 grant → 无论时间戳多老都保留。
- SSH key 级默认 file access policy 与 SSH key 本体独立管理，不受 grant cleanup 影响。

## 技术细节

### GrantInfoStore（`crates/bifrost-admin/src/remote_invoke/grant_info_store.rs`）

已实现的接口：

```rust
impl GrantInfoStore {
    pub fn new(data_dir: &Path) -> Self;
    pub fn load_for_relay(&self, relay_url: &str) -> Result<HashMap<String, GrantInfo>>;
    pub fn upsert(&self, relay_url: &str, grant_id: &str, info: &GrantInfo) -> Result<()>;
    pub fn remove(&self, grant_id: &str) -> Result<()>;
    pub fn retain_only(&self, relay_url: &str, grant_ids: &HashSet<String>) -> Result<()>;
}
```

`retain_only` 是 SSE 对账的收口点：`worker.rs::run_sse_session` 在 `fetch_active_grants` 成功返回后，把 relay 返回的 grant_id 集合传入，本地多余 grant 及其关联 crypto / policy 记录被批量清理。

### Worker 关键路径（`crates/bifrost-admin/src/remote_invoke/worker.rs`）

- `list_grants_and_cleanup()`（L1778）是唯一对外暴露的 grants 数据源，Admin handler / CLI 都调它，避免只在 UI 层隐藏。
- `validate_grant`（L3861 附近）在通过校验后写入 `last_command_at = now`；once grant 消耗时也同步写入。
- `normalize_registration_session_token`：token 为空或纯空白直接返回 None，worker 保持 `Disconnected`，只打一次 `INFO`。
- `list_grants_and_cleanup_prunes_stale_inactive_grants`（tests L7388）验证 48h stale cleanup；`list_grants_and_cleanup_keeps_stale_grant_with_active_call`（L7420）验证 active call 保护。

### Relay 回帧短重试（`crates/bifrost-admin/src/remote_invoke/relay_client.rs`）

- 仅 `post_call_frame` / `post_call_stream_frame` 的 request send error 触发退避 `150ms/500ms/1000ms`。
- HTTP 4xx/5xx、业务 `code != 0` 立即失败，不重试，避免掩盖权限或队列错误。

### Caller 侧 stale 清理（`crates/bifrost-cli/src/commands/remote.rs`）

- `find_reusable_grant` 返回 None → CLI 打印 `authorization expired, please run \`bifrost remote connect ...\` again`，并把对应 `LocalConnection` 从 `remote-connections.json` 移除。
- `open_call` 收到 relay `grant_session_token_invalid` → 走同一 stale 清理路径。

## CLI + Web + Admin API

### CLI

```
$ bifrost setting grant list
GRANT_ID    RELAY                      CLIENT           FIRST CONNECTED     LAST COMMAND
g-abc123    https://sync.example.com   Eden-MacBook     2026-06-30 10:12    2026-07-02 18:03
g-def456    https://sync.example.com   Eden-Studio      2026-07-01 09:00    -
```

### Admin API

```
GET /_bifrost/api/remote-invoke/grants
[
  {
    "grant_id": "g-abc123",
    "first_connected_at": 1782812345000,
    "last_command_at": 1782957794000,
    "first_authorized_at": 1782812345000,
    "last_used_at": 1782957794000,
    "grant_mode": "permanent",
    "active_call_ids": []
  }
]
```

字段说明：`first_authorized_at` / `last_used_at` 是兼容字段，值等同于 `first_connected_at` / `last_command_at`；老客户端不解析新字段仍可正常工作。

### Web UI

Settings → Remote Invoke → Grants Table：
- `Connected` 列展示 `first_connected_at`
- `Last command` 列展示 `last_command_at`，未执行过命令时展示 `-`
- 行内 badge 展示 `active`（正在跑 call）、`stale`（>48h 待清理）等状态

## Sync 边界

- `GrantInfoStore` 是本机 Target 状态，不参与 Relay 侧同步；Relay 只感知 grant 生命周期事件。
- `remote-connections.json` 由 Caller 本机维护，其他机器不同步。
- SSH key 本体（`SshKeyStore`）由用户显式选择是否 sync，见 `remote-invoke-ssh-shell-extension.md`。

## Phase 1 – 存储与对账

- 完成 `GrantInfoStore` 落盘、`retain_only` 对账、SSE 断线重连集成。
- worker 每次收到 relay active grants 后立即调用 `retain_only`。

## Phase 2 – 时间线与 API

- `GrantInfo` 结构新增 `first_connected_at` / `last_command_at` 字段。
- `validate_grant` 在真正下发命令时写 `last_command_at`。
- Admin API / CLI grant 列表返回并展示两个时间戳。

## Phase 3 – 韧性补丁

- Recent Calls E2E fixture 端口 fallback + 60s ready 预算。
- 未登录 sync session 日志降噪：单次 INFO + 退避。
- `post_call_frame` 发送层短重试。

## Phase 4 – Stale grant cleanup

- `list_grants_and_cleanup()` 加入 48h 无活动清理。
- Active call 白名单保护。
- SSH key 默认 policy 与 grant 解耦。

## 测试方案

### 单元测试（`crates/bifrost-admin/src/remote_invoke/worker.rs`）

- `test_normalize_registration_session_token_rejects_missing_or_empty`（L4624）
- `test_normalize_registration_session_token_trims_valid_token`（L4633）
- `test_validate_grant_rejects_missing_grant`（L4999）
- `test_validate_grant_accepts_active_permanent`（L5007，断言 `grants["g1"].last_command_at == Some(5000)`）
- `test_validate_grant_accepts_once_and_consumes`（L5020）
- `test_validate_grant_rejects_consumed_once`（L5034）
- `test_validate_grant_rejects_expired`（L5048）
- `test_validate_grant_rejects_revoked`（L5060）
- `test_validate_grant_accepts_not_yet_expired`（L5072）
- `list_grants_and_cleanup_returns_live_and_prunes_dead`（L7364）
- `list_grants_and_cleanup_prunes_stale_inactive_grants`（L7388）
- `list_grants_and_cleanup_keeps_stale_grant_with_active_call`（L7420）

### 计划中但尚未落地的单元测试（as of 2026-06-16）

- `test_grant_info_store_round_trip`
- `test_grant_info_store_remove`
- `test_sse_reconcile_purges_stale_grants`（production 已实现，仅缺直接覆盖测试）
- `post_call_frame_retries_send_failure_once`（生产代码位于 `relay_client.rs`）

### E2E 测试

- `e2e-tests/tests/test_remote_invoke_recent_calls_args_preview_e2e.sh`
  - 断言 pair-code 授权后 Grants API 出现 `first_connected_at`，`last_command_at` 为空。
  - 执行远程搜索命令后断言 `last_command_at` 非空且 `>= first_connected_at`。
  - 请求端口被占用时脚本自动解析 fixture 日志中的实际端口并继续。
  - CI 高负载下 60s 内拿到实际端口，超时前打印 mock server 日志。
- `e2e-tests/tests/test_remote_invoke_missing_sync_token_log_e2e.sh`（新）
  - 空 `BIFROST_DATA_DIR` 启动 Bifrost，断言只出现一条 waiting sync session token INFO。
- `e2e-tests/tests/test_remote_invoke_e2e.sh`
  - Target 重启后 `/api/remote-invoke/grants` 从 `503` 恢复 `200`，缺失 crypto 的本地 grants 自动清理。

### 真实场景测试 `human_tests/remote-invoke.md`

- Grants 时间字段回归：discovery → approve → 查看 Grants → 执行远程命令 → 再看 Grants 断言 `last command` 有值。
- Recent Calls 端口 fallback：预占 mock 请求端口 → 运行脚本 → 确认 fallback + 全部断言通过。
- 缺 sync session token 日志：使用空临时数据目录启动，确认单次 INFO、无重复 ERROR。
- Stale grant cleanup：构造超过 48h 未活动、近期活动、正在执行 active call 三种 grant，确认只 stale 一条被移除。

## Review/Fix/Test 闭环

### 第 1 轮

- Review：`grant_info_store.rs` 每次 upsert/remove 都必须落盘；`worker.rs::list_grants_and_cleanup` 不允许被前端二次覆盖过滤逻辑。
- Test：跑上述单元测试子集 + Recent Calls E2E。

### 第 2 轮

- Review：`normalize_registration_session_token` 分支覆盖 4 种输入（None / empty / whitespace / 有效 token）。
- Review：`post_call_frame` 退避实现是否遵守 150/500/1000ms 且 HTTP 错误不重试。
- Test：`bash e2e-tests/tests/test_remote_invoke_recent_calls_args_preview_e2e.sh` + `test_remote_invoke_missing_sync_token_log_e2e.sh`。

## 风险与决策

- **Grant 时间戳兼容性**：新增字段与旧字段并存，老客户端不解析新字段仍能工作。若未来要移除旧字段，需要发一次 breaking release 提示。
- **48h cleanup 阈值**：产品选择 48h 是平衡「短时间脱机不掉授权」与「Grants 列表可读性」；如后续用户反馈 > 48h 仍在使用同一 grant，可通过 config 提升到 72h。
- **未登录 sync session 降噪**：只压制 remote-invoke worker 的 ERROR，其他真实 relay 网络错误仍走 ERROR/WARN，不做全局静默。
- **Recent Calls fixture 60s ready 预算**：macOS shard 高并发下 Python 启动可能 >10s，60s 预算显著降低误判；成本是失败发现慢 50s，可接受。
