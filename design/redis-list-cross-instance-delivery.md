# 基于 Redis List 的跨实例 SSE 事件投递方案

> **状态（2026-07-03 复核）**：整体方案 **仍处于 planned（未落地）**。原文以 `bifrost-server-v4/app` 为目标实现仓位，但该目录早已随架构整合被移除；当前 Remote Invoke Relay 只在 `packages/bifrost-sync-server/` 单实例内存版本上运行，未引入 Redis，也没有实现本方案描述的 `pushToQueue/drainQueue/startQueuePoller/stopQueuePoller` 及 `ri:mq:*` Key 家族。本设计因此拆成“单实例现状（shipped）”与“多实例 Relay + Redis List 队列（planned）”两条主线，避免与真实代码继续脱节。

## 背景

### 单实例 Relay 现状（shipped，2026-07-03）

- 关键实现：`packages/bifrost-sync-server/src/remote-invoke/sse.ts`
- SSE 连接注册表都是进程内 `Map`：`clientStreams`、`callerStreams`、`pairingWatchers`。
- 事件缓冲仍然是本地 Map fallback：`callerEventBuffers`（`sse.ts:7`）和 `pairingEventBuffers`（`sse.ts:10`）；分别在 `pushToCallerStream`（`sse.ts:135`）和 `pushToPairingWatcher`（`sse.ts:191`）中写入，等对应 `register*` 调用时 flush 出去。
- 全仓 `grep -i redis packages/**/*.ts` 0 命中：sync-server 无 Redis client、无 Lua 脚本、无跨实例路由。
- Rust 侧发送方（`crates/bifrost-admin/src/handlers/remote_invoke.rs`）与之前 `bifrost-server-v4` 时代协议兼容，通过 HTTPS 上行事件，接口签名保持不变。

### 多实例 Relay 目标（planned）

一旦 Relay 部署到多实例（例如 TCE 组内多 Pod），单实例内存 Map 无法覆盖：任意 API 请求可能落到未持有目标 SSE 的实例。原设计给出的多实例方案假设 `bifrost-server-v4/app/helper/remoteInvokeSse.ts` 里的 `remoteDeliver()` HTTP 直连投递不可靠（Pod IP、防火墙、负载均衡），并因此选择用 Redis List 承担跨实例事件队列。此现象仍成立，但原文引用的路径 `bifrost-server-v4/**` 与函数 `httpPost / remoteDeliver / handleInternalPush / verifyInternalSecret / INSTANCE_ADDR / INTERNAL_SECRET` 在当前仓库中**均不存在**，因此“删除旧代码”一节无实际清理对象。

### 字节云 Alchemy Redis 约束

| 类别 | 支持 |
| --- | --- |
| String / Hash / List / Set / Sorted Set / Lua | 是 |
| Pub/Sub、Stream、Blocking (BLPOP/BRPOP) | 否 |

结论：多实例投递不能依赖 Pub/Sub 或 XREAD，只能走 List + 定时轮询 + Lua 原子操作。

## 用户目标验证清单

### 必须实现（planned）

- 每条 SSE 连接（client / caller / pairing 三类）都在 Redis 中拥有一个 `ri:mq:<type>:<id>` 的 List 队列，TTL 300s，与连接 keepalive 对齐。
- 任意 Relay 实例可以 `RPUSH` 事件到目标队列；持有 SSE 的实例通过 Lua 脚本原子 `LRANGE + DEL` 读取并写入 SSE 流。
- 保持同实例快速路径：本地 Map 命中即直接 `writeSseEvent`，不走 Redis。
- 事件严格 FIFO：`RPUSH` 尾部追加、`LRANGE 0 -1` 返回 `[oldest, ..., newest]`。
- Caller 事件缓冲统一到 Redis List：Caller 未连接时事件也可入队，连接后 flush；删除本地 `callerEventBuffers` 与相关 Redis String（`ri:caller_buf:*`）双写。
- Redis interface 扩展 `rpush / expire / eval`。
- SSE 注册启动 poller；注销停止 poller 并 `DEL` 队列。
- 单实例（local sync-server）保留内存 fast path，不启用 Redis。

### 必须不破坏

- Rust CLI 与 admin 端 API 接口签名不变（`bifrost remote ...`、`GET/POST /api/remote-invoke/*`）。
- Grant 注册、鉴权、加密（X25519 ECDH + HKDF + AEAD）语义不变。
- 单实例本地开发流程仍可跑通完整 pair → grant → call → exit。
- `keepalive` 仍写 `ri:online` / `ri:online_set` 用于在线判定；短期兼容保留 `ri:conn_*`，方案落地后再清理。

### 必须真实验证

- 多实例部署下同一 client 的 pairing、call_open、frame/exit 三类事件在跨 Pod 场景 100% 到达。
- 单实例部署下功能与延迟无回归。

## 产品语义

- Relay 是透明中继，事件生产/消费方仅通过 `clientInstanceId` / `callId` / `pairingId` 三个键定位队列。
- 队列**只属于连接**，连接断开后 `DEL` 队列，避免孤立数据。
- 事件顺序对下游可观测，无需依赖 Pub/Sub。
- 单实例本地开发保持零 Redis 依赖，方便 e2e 测试与本地闭环。

## 技术细节（planned 多实例）

### Key 规约

| 连接类型 | Key | TTL |
| --- | --- | --- |
| Client Stream | `ri:mq:client:{clientInstanceId}` | 300s |
| Caller Stream | `ri:mq:caller:{callId}` | 300s |
| Pairing Watcher | `ri:mq:pairing:{pairingId}` | 300s |

事件 payload：

```json
{ "event": "pairing_request", "data": {...}, "id": "optional", "ts": 1713600000000 }
```

### 生产者路径

```
pushToClient(clientInstanceId, event, data, id):
  1. 本实例 clientStreams Map 命中 → writeSseEvent → return true
  2. 未命中 → RPUSH ri:mq:client:{clientInstanceId} <json>
              EXPIRE ri:mq:client:{clientInstanceId} 300
              return true
```

`pushToCallerStream` / `pushToPairingWatcher` 同构，key 分别是 `ri:mq:caller:{callId}` / `ri:mq:pairing:{pairingId}`。

### 消费者路径

```
每个 SSE 注册函数（registerClientStream / registerCallerEventStream / registerPairingWatcher）：
  1. 把连接放入本实例 Map + Redis online 表
  2. startQueuePoller(redis, queueKey, res, POLL_INTERVAL_MS)
     - setInterval → EVAL drainQueue(queueKey)
     - 遍历返回的 items 按序 writeSseEvent
  3. 断开时 stopQueuePoller(timerId, redis, queueKey)
     - clearInterval + DEL queueKey
```

### Lua 脚本 drainQueue

```lua
local items = redis.call('LRANGE', KEYS[1], 0, -1)
if #items > 0 then
  redis.call('DEL', KEYS[1])
end
return items
```

### 轮询参数

| 参数 | 值 | 理由 |
| --- | --- | --- |
| `POLL_INTERVAL_MS` | 300ms | 感知 <0.5s，QPS 可控 |
| Queue TTL | 300s | 与 CONN_TTL 对齐 |

### Redis interface 扩展

```typescript
export interface Redis {
  get(key: string): Promise<string | null>;
  set(key: string, value: string, ...args: any[]): Promise<any>;
  del(key: string): Promise<any>;
  sadd(key: string, ...members: string[]): Promise<any>;
  srem(key: string, ...members: string[]): Promise<any>;
  smembers(key: string): Promise<string[]>;
  rpush(key: string, ...values: string[]): Promise<number>;      // 新增
  expire(key: string, seconds: number): Promise<number>;         // 新增
  eval(script: string, numkeys: number, ...args: (string | number)[]): Promise<any>;  // 新增
}
```

### 简化的 keepalive

- `ri:online:{id}`、`ri:online_set` 继续写入用于在线判断。
- `ri:conn_*`（含 `inst` 地址）在过渡期继续写入，后续清理。
- 新架构中 `INSTANCE_ADDR` / `INTERNAL_SECRET` 不再需要。

### CLI / Web / Admin API

- 本方案不新增 CLI 或 Web 入口，改动限于 Relay 服务层。
- Admin API `GET /_bifrost/api/remote-invoke/status`、`GET /api/remote-invoke/calls`、`POST /api/remote-invoke/pairing/*` 等接口签名不变。

### Sync 边界

- 本方案属于 Relay 内部投递机制，不涉及 Rules/Group/Values 同步。
- Redis 只作为投递层缓存，不作为长期状态源：Grant / Pair / Call 状态仍走 sync-server 现有存储路径。

## Phase 1 – Redis interface & 队列工具

- 扩展 `Redis` interface；新增 `pushToQueue / drainQueue / startQueuePoller / stopQueuePoller`。
- 单元测试覆盖 RPUSH/EXPIRE/EVAL 与 FIFO 顺序。

## Phase 2 – Push 侧改造

- 改造 `pushToClient` / `pushToPairingWatcher` / `pushToCallerStream`：本地 Map 快速路径 + Redis 队列 fallback。
- 删除 `callerEventBuffers`、`flushRedisCallerBuf`（旧多实例 fallback 已不存在，只需删除单实例 Map）。

## Phase 3 – Register/Unregister 与 poller 生命周期

- `registerClientStream` / `registerCallerEventStream` / `registerPairingWatcher` 启动 poller。
- `unregister*` 停 poller、`DEL` 队列。
- 打通 keepalive 写入 `ri:online*` 的语义。

## Phase 4 – 多实例部署与观测

- 灰度环境部署多实例 Relay，压测 pairing/call/frame 三类事件的跨实例送达率与端到端延迟。
- 单实例保持零 Redis 依赖回归通过。
- 更新 human_tests 索引与 Ops runbook。

## 测试方案

### 单元测试（planned）

- `pushToQueue_rpush_and_expire_are_atomic`
- `drainQueue_returns_items_in_fifo_order_and_deletes_key`
- `poller_writes_all_buffered_events_before_new_ones`
- `unregister_stops_timer_and_deletes_queue_key`
- `pushToClient_local_hit_bypasses_redis`

### E2E 测试

- 复用 `e2e-tests/tests/test_remote_invoke_e2e.sh`（当前存在的 sync-server 版本，覆盖完整闭环），保证单实例路径不回归。
- 多实例落地后新增 `test_remote_invoke_multi_instance.sh`，验证跨 Pod pairing/openCall/frame/exit 事件全部收敛。

### 真实场景测试（human_tests/remote-invoke.md）

- TC-RI-回归-98：多实例 Relay 下 pairing → approve → Caller 收到通知（走 `pushToPairingWatcher` + Redis List）。
- TC-RI-回归-99：多实例 Relay 下 openCall → 目标 client 收到 `call_open`（走 `pushToClient` + Redis List）。
- TC-RI-回归-100：多实例 Relay 下 frame/exit 事件 Caller 正确接收（走 `pushToCallerStream` + Redis List）。

上述 3 例目前均标记为 planned；多实例路径未 ship 前无法执行。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核 `pushToQueue` 是否在同一 EVAL 或者 pipeline 中完成 `RPUSH + EXPIRE`，避免 TTL 缺失。
- 复核 `drainQueue` 是否有 max 结果条数保护，防止 poller 一次拿爆内存。
- 复核 `callerEventBuffers` / `pairingEventBuffers` 删除后单实例本地开发是否仍能通过（因为单实例本身没有 Redis，需要保留内存缓冲）。
- 复测：Phase 1/2/3 单测、`test_remote_invoke_e2e.sh` 单实例。

### 第 2 轮

- 复核多实例部署下 Grant 撤销与 SSE 断连的顺序：`grant_revoked` 事件不能被落在 dead queue 里未消费。
- 复核 Redis Key 迁移策略：若上线时段 Relay 与 sync-server 混部，防止误把 sync-server 的 Redis 客户端加载到 sync-server 代码路径。
- 复测：多实例 e2e、TC-RI-回归-98/99/100 真实执行。

## 风险与决策点

- **单实例保留内存 fallback**：本设计明确 sync-server 单实例继续用 `callerEventBuffers`、`pairingEventBuffers`，避免为本地开发引入 Redis 依赖。多实例部署阶段这些本地缓冲可以保留作为“同实例快速缓冲”，与 Redis 队列不冲突。
- **Redis TTL 与断线补投**：断线后 300s 内新事件仍会入队并被下次 SSE 连接 poll 到；超时事件丢失，需要在客户端补一次 `bulk_state` 查询恢复。
- **多路径共存**：过渡期 `ri:conn_*` 依然会被 keepalive 写，`INSTANCE_ADDR` 字段保留但不再驱动路由；文档需明确“过渡期字段”，避免运维误以为仍需要保持内网直连。
- **未来演进**：若 Alchemy Redis 开放 Stream，可将 Redis List + 轮询替换为 XADD/XREAD 减少空轮询；接口层的 `pushToQueue/drainQueue` 抽象保证切换成本可控。

## 实现状态一览

| Step | 状态 | 说明 |
| --- | --- | --- |
| Redis interface 扩展 | planned | sync-server 未引入 Redis |
| 队列工具函数 | planned | 无 `pushToQueue / drainQueue / startQueuePoller / stopQueuePoller` |
| 改造 `pushToClient` | planned | 当前只查本地 Map，未命中直接返回 false |
| 改造 `pushToPairingWatcher` | planned | 未连接时退回 `pairingEventBuffers` |
| 改造 `pushToCallerStream` | planned | 未连接时退回 `callerEventBuffers`，旧 buffer 未清理 |
| 注册/注销启动/停止 poller | planned | `register*` / `unregister*` 不涉及 queue poller |
| 删除旧跨实例代码 | 不适用 | `bifrost-server-v4/*` 已不存在，`httpPost / remoteDeliver / handleInternalPush / verifyInternalSecret / INSTANCE_ADDR / INTERNAL_SECRET` 也不在当前仓库 |
| sync-server 兼容 | shipped | 单实例内存投递保持不变 |

### 验证结果

| 用例 | 状态 |
| --- | --- |
| TypeScript 编译（sync-server） | shipped：与本方案无关 |
| `cargo test --workspace` | 与本方案无直接关系 |
| TC-RI-回归-98 / 99 / 100 | planned：多实例 Relay 路径未实现，无法回归 |
