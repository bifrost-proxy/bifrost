# Remote Pairing Timeout Cleanup

> 状态：已交付并回归（2026-06-17 核对） | 关联：`design/remote-invoke-security-redesign.md`

## 背景

`bifrost remote connect` 的配对流程里，caller 发起 `pairings/start` 后如果用户没有及时审批，pairing 会在 relay 侧超时（默认 120s）。现网反馈表明，超时后的 pairing 曾在两条关键路径上残留：

1. `GET /v4/remote-invoke/client/pending-pairings` 仍返回已过期记录，Web UI 的 `Pending Pairing Requests` 长时间显示脏数据；
2. `startPairing()` 的 `pair_slot_occupied` 判定仍把这些过期记录算作活跃占槽，导致新的 pair code 连接被错误拒绝；
3. 用户点击 Approve / Reject 时，relay 侧没有统一把过期 pairing 收敛成 `expired`，admin 端会看到不稳定的 500 / 错误提示；
4. admin worker 本地按“收到时间 + 本地 `pair_code_ttl_secs`”做兜底淘汰，没有优先消费 relay `expires_at`，当 relay 真实 TTL 更短时，本地列表依然滞后。

这会直接破坏“超时后重新获取 code 并再次发起配对”的主链路可靠性。

## 用户目标验证清单

### 必须实现

- 过期 pairing 不再出现在 relay pending 列表中。
- 过期 pairing 不再占用 `pair_slot_occupied`，caller 可立即重发 pair code。
- 对已过期 pairing 的审批请求统一收敛为 `410 pairing_expired`，而不是 500。
- admin worker 收到 relay 的 stale error 后自动清理本地 `pending_pairings`，前端走 warning toast + 自动刷新分支。
- 本地兜底清理优先使用 relay `expires_at`（来自 SSE / poll pending），仅在缺失时回退到本地 TTL。

### 必须不破坏

- 未过期 pairing 的 approve / reject / polling / watcher 行为保持不变。
- SSE 事件顺序（`pairing_request` → `pairing_approved` / `pairing_rejected` / `pairing_expired`）不打乱。
- Rust 侧 `pending_pairings()` API 契约不变；仅内部先做清理。

### 必须真实验证

- vitest：`packages/bifrost-sync-server/src/__tests__/remote-invoke-pairing-timeout.test.ts` 两条用例。
- Rust 单测：`crates/bifrost-admin/src/remote_invoke/worker.rs::test_is_relay_stale_pairing_error_matches_expired_and_not_pending`（`~L5243`）。
- Human tests：`human_tests/remote-invoke.md` 的 TC-RI-回归-62 / 63。

## 产品语义

pairing 有 4 个状态：`pending_approval`、`approved`、`rejected`、`expired`。

- 只有 `pending_approval` + `expires_at > now` 的 pairing 才算“活跃”。
- 一旦过期，relay 端应主动把状态置为 `expired`，同时向 pairing watcher 推送 `expired` 事件、在 event log 追加 `pairing_expired`。
- 过期属于 relay 权威判定；admin worker 与 caller 侧收到 relay 的 stale error 后统一以“该 pairing 已 not found or expired”对外展示。

## 技术细节

### 1. Relay 查询面：`RemoteInvokeService`

实现位于 `packages/bifrost-sync-server/src/remote-invoke/service.ts`：

- `isPendingPairingExpired(pairing, nowMs)`（`:266`）：`status === 'pending_approval' && expires_at <= now`。
- `expirePendingPairing(pairing, reason='pairing_expired')`（`:274`）：DAO 置 `expired`、pushWatcher、append event log。
- `loadActivePendingPairings(clientInstanceId)`（`:297`）：内部驱动器，遍历 pending 列表，命中过期即 `expirePendingPairing()`，只返回仍活跃的项。

复用调用点：

- `getPendingPairingsForClient()`（`:323`）：前端拉列表前先清理。
- `cancelPendingPairings()`（`:343`）：避免把本应 `expired` 的旧记录继续按“活跃 pending”处理。
- `startPairing()`（`:373`）：`pair_slot_occupied` 判断前先清理。

### 2. Relay 审批面

`submitGrantDecision()`（`:453`）在 `pending_approval` 检查前调用 `isPendingPairingExpired(pairing)`；命中则 `expirePendingPairing(pairing)` 并 `throw pairing_expired`。

### 3. HTTP 状态码映射

`packages/bifrost-sync-server/src/routes/remote-invoke.ts` 里的错误映射：

| server error | HTTP |
|---|---|
| `pairing_expired` | `410 Gone` |
| `pairing_not_found` | `404 Not Found` |
| `pairing_not_pending` | `400` |
| `client_mismatch` | `403 Forbidden` |
| 其他 | `400 Bad Request` |

### 4. Admin worker 错误收敛

`crates/bifrost-admin/src/remote_invoke/worker.rs`：

- `is_relay_stale_pairing_error()`（`:3827`）识别 `pairing_expired` / `pairing_not_pending` / `pairing_not_found` 三种错误。
- `approve_pairing()`（`~L1013`）与 `reject_pairing()`（`~L1188`）在命中 stale 时：
  - `self.pending_pairings.write().remove(pairing_id)` 清理本地；
  - 统一转换成 `pairing <id> not found or expired`，供 handler 层展示。
- `pending_pairings()`（`:482`）返回列表前触发一次本地扫描；`poll_pending_pairings_from_relay()`（`:1610`）把 relay `expires_at` 持久化到 `TimestampedPairing`。
- `pairing_request_is_alive()`（`:3693`）优先使用 relay `expires_at`，仅在缺失时回退到 `pair_code_ttl_secs * 1000` 兜底。

### 5. Handler 状态码映射

`crates/bifrost-admin/src/handlers/remote_invoke.rs::pairing_action_status_code()`：

- stale pairing 错误 → `404 Not Found`（前端沿用现有 warning toast + 自动刷新分支）；
- 其余 → `500 Internal Server Error`。

## CLI + Web + Admin API

- CLI：无新增子命令；`bifrost remote connect` 在超时后可直接重发 pair code。
- Web UI：Pending Pairing Requests 列表在 stale 错误后自动移除对应项并 toast `pairing not found or expired`。
- Admin API：受影响端点仍是 `POST /admin/api/remote-invoke/pairings/:id/approve|reject` 与 `GET /admin/api/remote-invoke/pending-pairings`。

## Sync 边界

pairing 是 relay 存储对象，不参与本地 Bifrost sync；本设计只影响 relay 与 admin worker 的一致性，不改任何 rule / group sync 行为。

## Phase 拆分

### Phase 1：Relay 查询面清理

- `isPendingPairingExpired` / `expirePendingPairing` / `loadActivePendingPairings`。
- 三处调用点接入：`getPendingPairingsForClient`、`cancelPendingPairings`、`startPairing`。

### Phase 2：Relay 审批面拒绝

- `submitGrantDecision` 前置过期判定。
- HTTP 映射 `pairing_expired → 410`。

### Phase 3：Admin worker + Handler 收敛

- `is_relay_stale_pairing_error` + `approve/reject_pairing` 清理。
- handler 状态码映射。

### Phase 4：本地 TTL 与 SSE 时间戳

- 持久化 relay `expires_at`；`pairing_request_is_alive` 优先按 relay 判定。
- SSE 与 poll 双通道保持一致。

## 测试方案

### 单元测试

- `crates/bifrost-admin/src/remote_invoke/worker.rs::test_is_relay_stale_pairing_error_matches_expired_and_not_pending`：覆盖 `pairing_expired` / `pairing_not_pending` / `pairing_not_found`。
- 同文件其它测试覆盖本地 pending pairing 清理优先使用 relay `expires_at`。
- `crates/bifrost-admin/src/handlers/remote_invoke.rs`：stale pairing 错误映射为 `404`。

### Relay API E2E

`packages/bifrost-sync-server/src/__tests__/remote-invoke-pairing-timeout.test.ts`：

- `it('removes expired pending pairings from slot occupancy and pending list')`（`:223`）：验证过期 pending 不再占 `pair_slot_occupied`、不再出现在 `getPendingPairingsForClient`。
- `it('returns 410 for expired pairing decisions and persists expired status')`（`:297`）：验证 grant decision 对已过期 pairing 返回 `410`，DB 状态更新为 `expired`。

### Human Tests

`human_tests/remote-invoke.md`：

- TC-RI-回归-62：超时配对请求自动从 pending 列表中移除。
- TC-RI-回归-63：超时后点击 Authorize / Reject 显示友好错误并移除请求。

## Review/Fix/Test 闭环

### 第 1 轮

- 复核 relay 三个查询入口是否都过 `loadActivePendingPairings`。
- 复核 `submitGrantDecision` 前置检查顺序：`expired → status → decision`。
- 复测：pairing-timeout.test.ts、worker.rs stale 单测。

### 第 2 轮

- 复核 admin worker 本地清理与 `poll_pending_pairings_from_relay` 是否收敛一致。
- 复核 Web UI 在 stale toast 后自动刷新（无需人工刷新）。
- 复测：human_tests TC-RI-回归-62 / 63 端到端手动跑一遍。

## 风险与决策

| 风险 | 缓解 |
|---|---|
| relay 与 admin worker 之间存在轻微时间差 | 以 relay `expires_at` 为权威，admin worker 出现 stale error 时立即本地删除并 toast 引导刷新 |
| SSE watcher 未接收到 `pairing_expired` 事件 | poll pending 兜底会在下一次 `getPendingPairingsForClient` 触发时清理，最长滞后一个轮询周期 |
| Legacy caller 无 `expires_at` 字段 | worker 端 fallback `pair_code_ttl_secs * 1000`，行为回退到旧路径 |

## 实施状态（2026-06-17 核对）

本方案各项均已落地，关键代码位置：

- relay 查询面：`packages/bifrost-sync-server/src/remote-invoke/service.ts` 的 `expirePendingPairing() / loadActivePendingPairings()`，被 `getPendingPairingsForClient()` / `cancelPendingPairings()` / `startPairing()` 复用。
- relay 审批面：`submitGrantDecision()` 前置 `isPendingPairingExpired()` 检查。
- HTTP 状态码：`routes/remote-invoke.ts` 中 `pairing_expired → 410`、`pairing_not_found → 404`、`client_mismatch → 403`。
- admin worker：`is_relay_stale_pairing_error()` 覆盖三种错误；`approve_pairing()` / `reject_pairing()` 清理本地并转成统一提示。
- handler：`pairing_action_status_code()` 把 stale error 映射为 `404`。
- 本地兜底：`pairing_request_is_alive()` 优先使用 relay `expires_at`，缺失时回退到 `pair_code_ttl_secs * 1000`。

暂未发现“planned, not yet shipped as of 2026-06-17”项。

## 影响范围

- `packages/bifrost-sync-server/src/remote-invoke/service.ts`
- `packages/bifrost-sync-server/src/routes/remote-invoke.ts`
- `crates/bifrost-admin/src/remote_invoke/worker.rs`
- `crates/bifrost-admin/src/handlers/remote_invoke.rs`
- `packages/bifrost-sync-server/src/__tests__/remote-invoke-pairing-timeout.test.ts`
- `human_tests/remote-invoke.md`
