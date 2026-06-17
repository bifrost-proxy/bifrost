# Remote Pairing Timeout Cleanup

## 背景

`remote connect` 的配对流程里，caller 发起 `pairings/start` 后如果用户没有及时审批，pairing 会在 relay 侧超时。现网反馈表明，超时后的 pairing 仍然会残留在两条关键路径上：

- `GET /v4/remote-invoke/client/pending-pairings` 仍返回已过期记录，导致 Web UI 的 `Pending Pairing Requests` 长时间显示脏数据
- `startPairing()` 的 `pair_slot_occupied` 判定仍把这些过期记录算作活跃占槽，导致新的 pair code 连接被错误拒绝
- 用户点击 Approve / Reject 时，relay 侧没有统一把过期 pairing 收敛成 `expired`，admin 端就会看到不稳定的 500/错误提示

这会直接破坏“超时后重新获取 code 并再次发起配对”的主链路可靠性。

## 根因

relay 侧 `packages/bifrost-sync-server` 的配对状态语义不一致：

1. `listPendingPairings()` / `countPendingPairings()` 只按 `status='pending_approval'` 过滤，没有同时过滤 `expires_at`
2. `submitGrantDecision()` 只检查 `pending_approval`，没有在审批前判定 pairing 是否已经超时
3. admin worker 本地虽然会周期性清理 `pending_pairings`，但此前只按“收到时间 + 本地 `pair_code_ttl_secs`”做兜底淘汰，没有优先消费 relay 在 SSE / pending API 中返回的 `expires_at`
4. 因此当 relay 的真实 pairing TTL 比本地默认值更短时，本地 `Pending Pairing Requests` 仍会把已经过期的请求继续展示，直到本地 TTL 窗口走完

## 修复目标

1. 过期 pairing 不再出现在 relay pending 列表中
2. 过期 pairing 不再占用 `pair_slot_occupied`
3. 对已过期 pairing 的审批请求统一收敛为“not found or expired”，而不是 500
4. 保持现有未过期 pairing 的 approve / reject / polling / watcher 行为不变

## 实现方案

### 1. relay 统一在查询面清理过期 pending pairing

在 `RemoteInvokeService` 中新增统一的 pending pairing 收敛逻辑：

- 判断 `status === pending_approval && expires_at <= now`
- 将其状态更新为 `expired`
- 向 pairing watcher 推送 `expired`
- 在 event log 中追加 `pairing_expired`
- 仅把仍然有效的 pending pairing 返回给上层调用方

该逻辑会被以下路径复用：

- `getPendingPairingsForClient()`：避免前端继续看到脏数据
- `startPairing()`：在做 `pair_slot_occupied` 判断前先清理已过期 pairings
- `cancelPendingPairings()`：避免把本应 `expired` 的旧记录继续按“活跃 pending”处理

### 2. relay 在审批面显式拒绝过期 pairing

`submitGrantDecision()` 在检查 `pending_approval` 之前，先对目标 pairing 执行过期判定：

- 若已过期，则立即标记为 `expired`
- 返回 `pairing_expired`

`routes/remote-invoke.ts` 再将该错误映射为 `410 Gone`，明确告诉调用方“该资源已经失效”。

### 3. admin worker / handler 收敛 relay stale pairing 错误

为避免前端继续感知到 500：

- `RemoteInvokeWorker::approve_pairing()` / `reject_pairing()` 在 relay 返回 `pairing_expired`、`pairing_not_found`、`pairing_not_pending` 时，主动从本地 `pending_pairings` 移除该 pairing，并统一转成 `pairing <id> not found or expired`
- `handlers/remote_invoke.rs` 将这类错误映射为 `404`，让前端走现有“warning toast + 自动刷新列表”的友好分支
- `pending_pairings()` 在返回列表前先执行一次本地过期清理，减少定时器窗口内的脏数据可见性
- worker 在处理 `pairing_request` SSE 和 relay `pending-pairings` 轮询结果时，持久化 relay 下发的 `expires_at`，本地清理时优先按该时间戳收敛，而不是盲目套用默认 120 秒 TTL

## 测试方案

### 单元测试

- `crates/bifrost-admin/src/remote_invoke/worker.rs`
  - stale pairing 错误识别应覆盖 `pairing_expired` / `pairing_not_pending`
  - 本地 pending pairing 清理应优先使用 relay `expires_at`
- `crates/bifrost-admin/src/handlers/remote_invoke.rs`
  - stale pairing 错误应映射为 `404`

### Relay API E2E

- `packages/bifrost-sync-server/src/__tests__/remote-invoke-pairing-timeout.test.ts`
  - 过期 pending pairing 不应继续占用 `pair_slot_occupied`
  - expired pairing 的 grant decision 应返回 `410`，并将数据库状态更新为 `expired`

### Human Tests

- `human_tests/remote-invoke.md`
  - TC-RI-回归-62：超时配对请求自动从 pending 列表中移除
  - TC-RI-回归-63：超时后点击 Authorize/Reject 显示友好错误并移除请求

## 影响范围

- `packages/bifrost-sync-server/src/remote-invoke/service.ts`
- `packages/bifrost-sync-server/src/routes/remote-invoke.ts`
- `crates/bifrost-admin/src/remote_invoke/worker.rs`
- `crates/bifrost-admin/src/handlers/remote_invoke.rs`
- `packages/bifrost-sync-server/src/__tests__/remote-invoke-pairing-timeout.test.ts`
- `human_tests/remote-invoke.md`

## 实现状态（2026-06-17 核对）

本方案各项已经落地，关键代码位置：

- relay 查询面收敛过期 pairing：`packages/bifrost-sync-server/src/remote-invoke/service.ts` 中 `expirePendingPairing()` + `loadActivePendingPairings()`，分别被 `getPendingPairingsForClient()`、`cancelPendingPairings()`、`startPairing()` 复用，`pair_slot_occupied` 判定前会先剔除过期项。
- relay 审批面：`submitGrantDecision()` 在状态检查前调用 `isPendingPairingExpired()`，命中则 `expirePendingPairing()` 并 `throw pairing_expired`。
- HTTP 状态码映射：`packages/bifrost-sync-server/src/routes/remote-invoke.ts` 中 `pairing_expired → 410`、`pairing_not_found → 404`、`client_mismatch → 403`，其余 → 400。
- admin worker 错误收敛：`crates/bifrost-admin/src/remote_invoke/worker.rs` 的 `is_relay_stale_pairing_error()` 同时识别 `pairing_expired` / `pairing_not_pending` / `pairing_not_found`，`approve_pairing()` / `reject_pairing()` 命中后从本地 `pending_pairings` 删除并统一转成 `pairing <id> not found or expired`。
- handler 状态码映射：`crates/bifrost-admin/src/handlers/remote_invoke.rs` 的 `pairing_action_status_code()` 把 stale pairing 错误映射为 `404`，其余为 `500`。
- 本地清理优先使用 relay `expires_at`：`pairing_request_is_alive()` 优先用 `request.expires_at`（来自 SSE / `poll_pending_pairings_from_relay()` 持久化的 relay 时间戳），仅在缺失时回退到 `pair_code_ttl_secs * 1000` 兜底。

暂未发现“(planned, not yet shipped as of 2026-06-17)”项。
