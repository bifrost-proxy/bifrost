# 基于 Redis List 的跨实例 SSE 事件投递方案

> **状态（2026-06-16）**：本方案整体 **(planned, not yet shipped as of 2026-06-16)**。原文以 `bifrost-server-v4/` 为目标实现仓位，但该目录已不在仓库中——Remote Invoke Relay 已迁移到 `packages/bifrost-sync-server/`，当前是单实例 in-memory 实现（详见第七节「当前代码现状」）。本节描述的多实例 Relay 部署形态仍是规划目标，并未在生产中落地。

## 一、问题背景

### 当前架构

> (planned, not yet shipped as of 2026-06-16) 下述「多实例 Relay + 跨实例投递」是设计前提，而非当前部署形态。

bifrost-server-v4 (Relay) 多实例部署在负载均衡后面。SSE 事件投递依赖三条通道：

| 通道 | 用途 | Key |
|---|---|---|
| `pushToClient` | Relay → 执行端 (Bifrost Server) | `clientInstanceId` |
| `pushToCallerStream` | Relay → 调用端 (CLI Caller) | `callId` |
| `pushToPairingWatcher` | Relay → 配对等待端 (CLI Caller) | `pairingId` |

每条通道的投递流程：
1. 检查本实例内存 Map → 同实例直接写入 SSE 流
2. 不在本实例 → `remoteDeliver()` → 从 Redis 读取目标实例地址 → HTTP POST 到 `http://{inst}/internal/ri-push`

### 问题根因

`remoteDeliver` 的 HTTP POST 跨实例投递**不可靠**：
- Pod 内网地址路由不通 / 防火墙限制
- `INSTANCE_ADDR` 配置错误（`MY_POD_IP` 环境变量缺失或不正确）
- 负载均衡器不支持 Pod 间直连
- 导致 Caller 无法收到授权结果、执行端无法收到 `call_open` 等关键事件

### 约束条件（字节云 Alchemy Redis）

| 类别 | 支持情况 |
|---|---|
| String (GET/SET/INCR...) | ✅ 支持 |
| Hash (HGET/HSET/HGETALL...) | ✅ 支持 |
| List (LPUSH/RPUSH/LPOP/RPOP/LRANGE/LLEN) | ✅ 支持 |
| Set (SADD/SREM/SMEMBERS...) | ✅ 支持 |
| Sorted Set (ZADD/ZRANGE...) | ✅ 支持 |
| Lua (EVAL/EVALSHA) | ✅ 支持 |
| **Pub/Sub (SUBSCRIBE/PUBLISH)** | ❌ 不支持 |
| **Stream (XADD/XREAD)** | ❌ 不支持 |
| **Blocking (BLPOP/BRPOP)** | ❌ 不支持 |

## 二、设计方案

### 核心思路：Redis List 作为 Per-Connection 事件队列

为每个 SSE 连接在 Redis 中维护一个 List 作为事件队列：
- **生产者**（任意实例）：将事件 RPUSH 到目标连接的 Redis List
- **消费者**（持有 SSE 连接的实例）：定时轮询 LPOP 事件并写入 SSE 流

```
┌─────────────┐     RPUSH           ┌──────────────┐     LPOP          ┌─────────────┐
│ Instance A   │ ──────────────────▶ │ Redis List    │ ◀──────────────── │ Instance B   │
│ (API 请求)   │                     │ ri:mq:client: │                   │ (SSE 连接)   │
│              │                     │ {clientId}    │                   │              │
└─────────────┘                     └──────────────┘                   └─────────────┘
```

### 事件队列 Key 命名

| 连接类型 | Redis Key | TTL |
|---|---|---|
| Client Stream | `ri:mq:client:{clientInstanceId}` | 300s |
| Caller Stream | `ri:mq:caller:{callId}` | 300s |
| Pairing Watcher | `ri:mq:pairing:{pairingId}` | 300s |

### 事件消息格式

```json
{
  "event": "pairing_request",
  "data": { ... },
  "id": "optional_event_id",
  "ts": 1713600000000
}
```

### 投递流程

#### Push（生产者）

```
pushToClient(clientInstanceId, event, data, id):
  1. 检查本实例 clientStreams Map
     → 找到 → writeSseEvent() → return true (同实例快速路径)
  2. 不在本实例 → RPUSH ri:mq:client:{clientInstanceId} JSON({event, data, id, ts})
     → EXPIRE ri:mq:client:{clientInstanceId} 300
     → return true
```

#### Poll（消费者）

```
每个 SSE 连接启动后:
  1. 注册到本实例内存 Map + Redis（保留现有逻辑，用于 keepalive）
  2. 启动定时器 setInterval(pollFn, POLL_INTERVAL_MS)
  3. pollFn:
     - EVAL Lua 脚本: 原子读取并清空队列
       local items = redis.call('LRANGE', KEYS[1], 0, -1)
       if #items > 0 then redis.call('DEL', KEYS[1]) end
       return items
     - 遍历 items，按顺序 writeSseEvent()
  4. SSE 断开时: clearInterval + DEL 队列 Key
```

### 轮询间隔

| 参数 | 值 | 理由 |
|---|---|---|
| `POLL_INTERVAL_MS` | 300ms | 在延迟（用户感知 < 0.5s）和 Redis 负载之间取平衡 |
| 队列 TTL | 300s | 与 CONN_TTL 一致，防止孤立队列 |

### Lua 脚本

**drainQueue（原子读取+清空）：**
```lua
local items = redis.call('LRANGE', KEYS[1], 0, -1)
if #items > 0 then
  redis.call('DEL', KEYS[1])
end
return items
```

> RPUSH 保证插入顺序（尾部追加），LRANGE 0 -1 返回 [oldest, ..., newest]，事件按正确的时间顺序投递。

### Caller Event Buffer 简化

**当前架构**中 `pushToCallerStream` 有复杂的本地 + Redis 双重缓冲（`callerEventBuffers` Map + `ri:caller_buf:{callId}` String），原因是 Caller 可能在事件产生后才连接 SSE。

**新架构**中 Redis List 天然充当事件缓冲：
- 事件 RPUSH 到 `ri:mq:caller:{callId}`，无论 Caller 是否已连接
- Caller 连接后开始 LPOP，自然获取所有缓冲事件
- **不再需要** `callerEventBuffers` Map 和 `ri:caller_buf` Key

### 可删除的代码

| 模块 | 删除内容 | 原因 |
|---|---|---|
| `remoteInvokeSse.ts` | `httpPost()` 函数 | 不再需要实例间 HTTP 通信 |
| `remoteInvokeSse.ts` | `remoteDeliver()` 函数 | 被 Redis List RPUSH 替代 |
| `remoteInvokeSse.ts` | `handleInternalPush()` 函数 | 不再接收 HTTP 推送 |
| `remoteInvokeSse.ts` | `verifyInternalSecret()` 函数 | 无内部 HTTP 通信 |
| `remoteInvokeSse.ts` | `callerEventBuffers` Map 和相关逻辑 | Redis List 即缓冲 |
| `remoteInvokeSse.ts` | `flushRedisCallerBuf()` 函数 | 同上 |
| `remoteInvokeSse.ts` | `INSTANCE_ADDR` 常量 | 不再需要标识实例地址做投递 |
| `remoteInvokeSse.ts` | `INTERNAL_SECRET` 常量 | 无内部认证需求 |
| `routes/remoteInvoke.ts` | `/internal/ri-push` 路由 | 被 Redis List 替代 |
| `router.ts` | `/internal/ri-push` 路由（重复定义） | 同上 |

### Redis 接口扩展

当前自定义 `Redis` 接口需新增以下方法：

```typescript
export interface Redis {
  // 现有方法...
  get(key: string): Promise<string | null>;
  set(key: string, value: string, ...args: any[]): Promise<any>;
  del(key: string): Promise<any>;
  sadd(key: string, ...members: string[]): Promise<any>;
  srem(key: string, ...members: string[]): Promise<any>;
  smembers(key: string): Promise<string[]>;
  // 新增方法
  rpush(key: string, ...values: string[]): Promise<number>;
  expire(key: string, seconds: number): Promise<number>;
  eval(script: string, numkeys: number, ...args: (string | number)[]): Promise<any>;
}
```

### keepalive 中连接信息的简化

当前 keepalive 中每 30s 写入 `ri:conn_client:{id}` → `{inst: INSTANCE_ADDR, stream_id}` 用于 `remoteDeliver` 查找目标实例。

新架构不再需要 `inst` 地址做投递路由，但 **保留** Redis 连接信息写入：
- 用于判断客户端在线状态（`ri:online:{id}`, `ri:online_set`）
- 用于其他实例判断连接是否存活（查询用，不做投递）
- 可简化为只写 `ri:online` 和 `ri:online_set`，不再写 `ri:conn_*`

暂时保留 `ri:conn_*` 写入以保持兼容，后续可清理。

## 三、实现步骤

### Step 1: 扩展 Redis 接口
- 在 `remoteInvokeSse.ts` 的 `Redis` interface 中新增 `rpush`, `expire`, `eval`

### Step 2: 新增 Redis List 消息队列工具函数
- `pushToQueue(redis, queueKey, event, data, id?)`: RPUSH + EXPIRE
- `drainQueue(redis, queueKey)`: EVAL Lua 原子读取+清空
- `startQueuePoller(redis, queueKey, res, intervalMs)`: 返回 timer ID
- `stopQueuePoller(timerId, redis, queueKey)`: clearInterval + DEL key

### Step 3: 改造 pushToClient
- 保留本地 Map 快速路径
- 将 `remoteDeliver` 替换为 `pushToQueue(redis, 'ri:mq:client:' + clientInstanceId, ...)`

### Step 4: 改造 pushToPairingWatcher
- 保留本地 Map 快速路径
- 将 `remoteDeliver` 替换为 `pushToQueue(redis, 'ri:mq:pairing:' + pairingId, ...)`

### Step 5: 改造 pushToCallerStream
- 保留本地 Map 快速路径
- 将 `remoteDeliver` + `callerEventBuffers` 替换为 `pushToQueue(redis, 'ri:mq:caller:' + callId, ...)`
- 删除 `callerEventBuffers` 相关代码

### Step 6: 在 SSE 注册函数中启动轮询
- `registerClientStream`: 调用 `startQueuePoller`，保存 timer ID 到 state 中
- `registerCallerEventStream`: 调用 `startQueuePoller`，删除旧的 flush 逻辑
- `registerPairingWatcher`: 调用 `startQueuePoller`

### Step 7: 在 SSE 注销函数中停止轮询
- `unregisterClientStream`: 调用 `stopQueuePoller`
- `unregisterCallerEventStream`: 调用 `stopQueuePoller`
- `unregisterPairingWatcher`: 调用 `stopQueuePoller`

### Step 8: 删除旧的跨实例投递代码
- 删除 `httpPost`, `remoteDeliver`, `handleInternalPush`, `verifyInternalSecret`
- 删除 `INSTANCE_ADDR`, `INTERNAL_SECRET` 常量
- 删除 `callerEventBuffers` Map 和 `flushRedisCallerBuf`
- 删除 `router.ts` 和 `routes/remoteInvoke.ts` 中的 `/internal/ri-push` 路由
- 移除 `routes/remoteInvoke.ts` 中对 `handleInternalPush`, `verifyInternalSecret` 的 import

### Step 9: 同步更新 bifrost-sync-server（本地开发用）
- 在 `packages/bifrost-sync-server` 的 SSE 投递中保持现有的单实例直接投递逻辑
- 无需改造（单实例无跨实例问题）

## 四、影响范围

### 改动文件

| 文件 | 改动类型 | 改动说明 |
|---|---|---|
| `bifrost-server-v4/app/helper/remoteInvokeSse.ts` | **核心重构** *(planned, not yet shipped as of 2026-06-16；该路径已不存在；当前对应实现位于 `packages/bifrost-sync-server/src/remote-invoke/sse.ts`)* | 替换投递机制，新增队列工具函数，删除 HTTP 投递代码 |
| `bifrost-server-v4/app/routes/remoteInvoke.ts` | 删除路由 *(planned, not yet shipped as of 2026-06-16；该路径已不存在)* | 移除 `/internal/ri-push` 路由和相关 import |
| `bifrost-server-v4/app/router.ts` | 删除路由 *(planned, not yet shipped as of 2026-06-16；该路径已不存在)* | 移除 `/internal/ri-push` 路由和相关 import |
| `bifrost-server-v4/app/service/remoteInvoke.ts` | 无需修改 *(planned, not yet shipped as of 2026-06-16；该路径已不存在)* | Service 层调用的 `pushToClient` 等接口签名不变 |

### 不影响的部分

- Rust 客户端代码（接口不变）
- Relay API 接口（对外 HTTP API 不变）
- 认证、注册、Grant 管理等逻辑
- bifrost-sync-server（单实例，无跨实例问题）

## 五、性能分析

| 指标 | 当前架构 | 新架构 |
|---|---|---|
| 同实例延迟 | ~0ms（内存直写） | ~0ms（不变） |
| 跨实例延迟 | 不可靠（HTTP 经常失败） | ≤300ms（轮询间隔） |
| Redis 额外开销 | 无（只做连接注册） | 每连接 3.3 QPS（300ms 轮询 EVAL） |
| 跨实例可靠性 | ❌ 不可靠 | ✅ 可靠（Redis 持久化队列） |
| 事件缓冲 | 本地 Map + Redis String | Redis List（天然缓冲） |
| 代码复杂度 | 高（双缓冲 + HTTP 投递） | 低（统一的队列 Push/Poll） |

## 六、验证计划

### 单元测试
- 验证 `pushToQueue` 正确写入 Redis List
- 验证 `drainQueue` Lua 脚本正确读取并清空队列
- 验证事件排序（FIFO）
- 验证队列 TTL 设置

### E2E 测试
- 已有 `test_remote_invoke_e2e.sh` 覆盖完整配对→授权→调用→退出流程
- 本地 sync-server 模式下功能不受影响

### 真实场景测试（human_tests）
- 在 `human_tests/remote-invoke.md` 新增回归用例：
  - TC-RI-回归-98: 远端 Relay 多实例下配对→授权→Caller 收到通知（验证 pushToPairingWatcher）
  - TC-RI-回归-99: 远端 Relay 多实例下 openCall→执行端收到 call_open（验证 pushToClient）
  - TC-RI-回归-100: 远端 Relay 多实例下 frame/exit 事件 Caller 正确接收（验证 pushToCallerStream）

## 七、实现状态

**(planned, not yet shipped as of 2026-06-16)**

### 当前代码现状（核对 2026-06-16）

- 原文中引用的 `bifrost-server-v4/` 目录已不存在于本仓库。Remote Invoke 的 Relay 实现已迁移到 `packages/bifrost-sync-server/`，SSE 投递入口位于 `packages/bifrost-sync-server/src/remote-invoke/sse.ts`。
- 该实现是**单实例内存版本**：所有 `clientStreams` / `callerStreams` / `pairingWatchers` 都是进程内 `Map`，没有引入 Redis（仓库 `packages/**/*.ts` 范围内 `grep -i redis` 0 命中）。
- 因此本设计文档中所有「跨实例」相关结构都未落地：没有 `Redis` interface、没有 `rpush/expire/eval`、没有 `pushToQueue/drainQueue/startQueuePoller/stopQueuePoller`、没有 `ri:mq:*` Key、没有定时轮询。
- 同样，文档中列为「需要删除」的旧跨实例代码（`httpPost` / `remoteDeliver` / `handleInternalPush` / `verifyInternalSecret` / `INSTANCE_ADDR` / `INTERNAL_SECRET` / `/internal/ri-push` 路由）在当前 sync-server 版本中**本来就不存在**，无需清理。
- 与本设计预期相反，`callerEventBuffers`（caller 事件本地缓冲 Map）目前仍然存在并被使用，见 `sse.ts:7,104,111,120,124,129`；只是它现在只是一个进程内 fallback buffer，不再与 Redis String 双写。
- `pushToCallerStream` / `pushToPairingWatcher` 在「目标 SSE 还没连上」时会把事件追加到本地 buffer Map（`callerEventBuffers` / `pairingEventBuffers`），等对应 `register*` 调用时再 flush 出去；这是单实例下对原方案「Redis List 充当缓冲」的等价简化版本。

### 实现记录

| Step | 状态 | 说明 |
|------|------|------|
| Step 1: 扩展 Redis 接口 | (planned, not yet shipped as of 2026-06-16) | sync-server 未引入 Redis，无 `Redis` interface |
| Step 2: 队列工具函数 | (planned, not yet shipped as of 2026-06-16) | 无 `pushToQueue` / `drainQueue` / `startQueuePoller` / `stopQueuePoller` |
| Step 3: 改造 pushToClient | (planned, not yet shipped as of 2026-06-16) | 当前 `pushToClient` 仅查本实例 `clientStreams` Map，目标不在本实例直接返回 false |
| Step 4: 改造 pushToPairingWatcher | (planned, not yet shipped as of 2026-06-16) | 未连上时退回本地 `pairingEventBuffers` Map，无 Redis |
| Step 5: 改造 pushToCallerStream | (planned, not yet shipped as of 2026-06-16) | 未连上时退回本地 `callerEventBuffers` Map，无 Redis；旧 buffer 未删除 |
| Step 6: 注册函数启动轮询 | (planned, not yet shipped as of 2026-06-16) | `register*` 函数不启动任何 queue poller |
| Step 7: 注销函数停止轮询 | (planned, not yet shipped as of 2026-06-16) | `unregister*` 函数不涉及 queue poller |
| Step 8: 删除旧代码 | 不适用 | 列举的旧跨实例符号在迁移后的 sync-server 中本来就不存在 |
| Step 9: bifrost-sync-server | 部分 | 维持单实例 in-memory 投递，但本方案中的 Redis 改造尚未落地 |

### 验证结果

本方案尚未实现，下列测试用例均未执行：

| 测试用例 | 结果 | 说明 |
|----------|------|------|
| TypeScript 编译 | (planned, not yet shipped as of 2026-06-16) | 当前 `sse.ts` 可正常编译，但与本方案无关 |
| cargo test --workspace | (planned, not yet shipped as of 2026-06-16) | 与本设计无直接关系 |
| TC-RI-回归-98 | (planned, not yet shipped as of 2026-06-16) | 多实例 Relay 路径未实现，无法回归 |
| TC-RI-回归-99 | (planned, not yet shipped as of 2026-06-16) | 同上 |
| TC-RI-回归-100 | (planned, not yet shipped as of 2026-06-16) | 同上 |
