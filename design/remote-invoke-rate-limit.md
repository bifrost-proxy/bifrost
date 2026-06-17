# Remote Invoke 限流分层改造

## 背景

`remote search`、`traffic.search` 等流式 remote invoke 调用在执行期间会持续产生：

- client -> relay `POST /v4/remote-invoke/client/calls/:callId/frame`
- caller -> relay `GET /v4/remote-invoke/calls/:callId/events`
- caller 取消时 `POST /v4/remote-invoke/calls/:callId/cancel`
- client 收尾时 `POST /v4/remote-invoke/client/calls/:callId/exit`

当前本地 relay `packages/bifrost-sync-server` 在所有业务路由前统一套用 `RateLimiter(200, 60_000)` 的 IP 固定窗口限流。这样高频 `frame` 会把整条调用链的额度吃光，导致 `events/cancel/exit` 这些终态控制请求也被 429 拒绝，出现：

- target client 已经进入 `cancelled`
- caller 侧仍卡住等待，或者误判为 `failed`

线上 relay `bifrost-server-v4` 的 `remoteInvoke.ts` 内部原本没有同样的应用层全站 limiter，因此线上出现的 `429` 更可能来自网关层。无论来源在哪，remote invoke 都需要把流式数据面和终态控制面拆分处理，避免最基础的取消能力被误伤。

## 目标

1. `cancel`、`events`、`exit` 等终态收尾路径不再被粗粒度全局限流打断
2. 大流量 `search.get` / `traffic.search` 不会挤占 caller 收尾能力
3. caller 在 relay 返回 429 或终态事件丢失时，仍能在合理时间内完成取消收尾
4. 为后续按路由/按 token/按 call_id 的细粒度限流留出结构化入口

## 最小止血方案

### 1. 本地 relay：从全局 IP limiter 中豁免全部 remote invoke 路径

在 `packages/bifrost-sync-server/src/index.ts` 对所有 `/v4/remote-invoke/*` 路径跳过统一 `globalLimiter`。

原因：

- remote invoke 已经有自己的一套匿名入口、caller、client、call 级别语义
- 同公司 NAT / 出口 IP 下的多个用户，不应因为共用出口 IP 而互相打爆
- 继续把 authenticated remote invoke 放在全站 IP 桶里，会让“共享出口 IP”成为系统性误伤点

### 2. remote invoke 内部改为按身份/资源分桶

在 `packages/bifrost-sync-server/src/routes/remote-invoke.ts` 内部新增 dedicated limiter：

- client 注册：按 `x-bifrost-token + client_instance_id`
- client 查询/管理接口：按 `client_instance_id`
- client `frame`：按 `call_id`
- caller `grants/reusable`：按 `bearer/relay_token + client_instance_id + caller_fingerprint`
- caller `calls/open`：按 `grant_id + client_instance_id + caller_fingerprint`
- caller `events/cancel/input`：按 `call_id`
- 匿名 `pairings/start`：仍按 IP

其中 `client/stream` 的共享连接上限从“按 IP”改为“优先按 user_id，缺失时才回退 IP”。

远端 relay `bifrost-server-v4/app/routes/remoteInvoke.ts` 不适合直接照搬本地 relay 的内存分桶 limiter。原因：

- 远端 relay 是多 pod 部署，pod 内计数器天然不一致
- remote invoke 长链路可能跨 pod 命中不同副本，pod-local limiter 会把同一条调用切成多份不稳定配额
- 远端 relay 运行在网关后面，容器内看到的 `ctx.ip` / `x-forwarded-for` 并不适合作为 authenticated remote invoke 的主要限流键

因此远端 relay 当前采用的更稳妥策略是：

- authenticated remote invoke 主链路在 app 内不再新增 pod-local rate limit
- `client/stream` 直接从 `verifyClientAuth()` 返回的 client owner 信息中写入 `user_id`，不再依赖 query 参数透传，避免后续任何连接上限逻辑再次被共享出口 IP 误导
- 若未来必须对远端 relay 做 authenticated remote 的限流，应使用 Redis / 网关级别的全局分桶，并按 `call_id / client_instance_id / grant_id + caller_fingerprint` 这类资源键限流，而不是按 IP
- 继续保留已经存在且适合分布式场景的资源级限制，例如 `ssh/challenge` 按 `device_code` 的 Redis 计数

### 3. caller：取消收尾增加 429 / 超时兜底

在 `crates/bifrost-cli/src/commands/remote.rs` 中：

- 正常执行路径继续使用长超时 SSE 收流
- 收到 `Ctrl-C` 后：
  - 先发送 `cancel_call`
  - 再进入短超时 `settle_cancelled_call()` 收尾
- 如果 `events` 在取消收尾阶段返回 429，做短重试
- 如果 cancel 已成功发出，但之后的 `events` 因 429 / 超时 / 其他收尾异常而无法确认终态，则合成一个 caller 侧 `cancelled` 结果，避免 CLI 无限挂住

该兜底只在“caller 已经主动取消，且 cancel 请求已成功发出”的上下文中生效，不影响普通成功路径。

## 后续完整方案

### 路由分桶（planned, not yet shipped as of 2026-06-16）

当前 `packages/bifrost-sync-server/src/routes/remote-invoke.ts` 已经按用途拆出 `registerLimiter` / `clientQueryLimiter` / `clientDataLimiter` / `clientStreamFrameLimiter` / `callerLookupLimiter` / `callerOpenLimiter` / `callerControlLimiter` 等多个 bucket，但这些 limiter 仍然是“以用途命名 + 各自维护键”的零散结构，并未抽成下面这套统一的语义类别。后续应进一步收敛成显式语义分桶：

- `anonymous_entry`
  - `pairings/start`
  - `ssh/challenge`
- `sse_handshake`
  - `client/stream`
  - `pairings/:id/watch`
  - `calls/:id/events` 建连
- `data_plane`
  - `client/calls/:id/frame`
  - `client/calls/:id/stream-frame`
- `terminal_control`
  - `calls/:id/cancel`
  - `client/calls/:id/exit`
  - 可选：`client/heartbeat`

### 限流维度

- 匿名入口：按 IP（`pairings/start` 仍走 `pairRateLimiters` 的 per-IP 计数；`ssh/challenge` 由远端 relay 的 Redis 计数兜底）
- 已授权 transport：按 `call_id` / `client_instance_id` / `grant_id`（已落地，键的拼装见 `applyRateLimit` 的 24 处调用）
- SSE 建连：按 `call_id` + 活跃连接数（活跃连接数已通过 `handleClientStream` 中的 `max_sse_connections_per_client` / `max_sse_connections_per_ip` 限制；按 `call_id` 的额外建连频率桶尚未落地，planned, not yet shipped as of 2026-06-16）
- 终态控制：高优先级、小流量、保底通道（已通过 `callerControlLimiter` 分离 `cancel/events/exit/input`）
- 多用户共享 NAT：不再把“出口 IP 相同”当成已认证 remote invoke 的主要限流依据（已在 `handleClientStream` 中改为 `user:<user_id>` 优先、回退 IP）

### 算法（planned, not yet shipped as of 2026-06-16）

`packages/bifrost-sync-server/src/security.ts` 的 `RateLimiter` 当前仍是固定窗口实现（`{count, resetAt}` + `windowMs`），未升级到 token bucket / leaky bucket。后续应在不破坏现有键空间的前提下替换底层算法，避免“59 秒打满，接下来整段时间全 429”的悬崖效应。

## 测试方案

### 单元测试

- `remote.rs`
  - 取消收尾阶段命中 `429` 时识别为可重试错误
  - 取消收尾阶段若只得到空默认结果，会合成 `cancelled`
- `packages/bifrost-sync-server`
  - 同一 `x-forwarded-for` 下的 authenticated remote invoke 请求不会被全局 IP limiter 打断（planned, not yet shipped as of 2026-06-16；现有 `src/__tests__/rate-limit.test.ts` 只覆盖了非 remote-invoke 路径的全局限流与 `auth_rate_limit_per_ip` 两条用例，仍需补这条 remote-invoke 豁免的回归用例）
  - 同一 `x-forwarded-for` 下不同用户的 `client/stream` 不会互相挤掉（planned, not yet shipped as of 2026-06-16；该场景目前由 `human_tests/remote-invoke.md` 的 TC-RI-回归-118 人工回归覆盖，尚未在 sync-server 的 vitest 套件中沉淀为自动化用例）
  - 非 remote invoke 路径仍继续走全局限流（已由 `src/__tests__/rate-limit.test.ts` 的 `uses server.rate_limit_per_ip for non remote-invoke paths` 用例覆盖）

### E2E

- `test_remote_invoke_e2e.sh`
  - 大量 `search.get` frame 产生时，caller `Ctrl-C`
  - 断言 client Recent Calls 最终为 `cancelled`
  - 断言 caller 不会无限挂起

### Human Tests

更新 `human_tests/remote-invoke.md`：

- 回归：粗粒度限流不再把 `cancel/events/exit` 打成 429
- 回归：线上 relay 下 caller `Ctrl-C` 后 target client 进入 `cancelled`
- 回归：caller 侧在取消后可在合理时间退出
- 回归：公司 NAT / 共享出口 IP 下，已认证 remote invoke 不再互相限流
- 回归：远端 relay `client/stream` 的在线状态不再依赖 query 透传 `user_id`
- 回归：远端 relay 不引入 pod-local authenticated remote limiter，避免多 pod / 网关后部署下的不一致限流
