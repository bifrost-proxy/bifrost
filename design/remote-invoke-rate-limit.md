# Remote Invoke 限流分层改造

> **状态**：本地 relay 分桶限流已落地;算法升级(token bucket)与部分回归 vitest 用例仍为 planned。
> **关联**:`design/remote-invoke-call-cancel.md`、`design/remote-invoke-call-history-persistence.md`、`design/remote-invoke-file-api.md`、`design/grant-file-access.md`。

## 背景

`remote search`、`traffic.search`、`shell.exec`、`file.read` 等 remote invoke 调用在执行期间会跨 caller → relay → target client 三段状态机维持长时间的连续请求:

- client → relay `POST /v4/remote-invoke/client/calls/:callId/frame`(流式数据面)
- client → relay `POST /v4/remote-invoke/client/calls/:callId/stream-frame`(大流量数据面)
- caller → relay `GET /v4/remote-invoke/calls/:callId/events`(SSE 事件面)
- caller → relay `POST /v4/remote-invoke/calls/:callId/cancel`(取消)
- client → relay `POST /v4/remote-invoke/client/calls/:callId/exit`(终态汇报)

历史实现里,本地 relay `packages/bifrost-sync-server/src/index.ts` 在所有业务路由前统一套用 `RateLimiter(rate_limit_per_ip, 60_000)` 的**全站 IP 固定窗口限流**。高频 `frame` 请求会把整条调用链的额度吃光,导致 `events` / `cancel` / `exit` 这些终态控制请求也被 429 拒绝,出现:

- target client 已经进入 `Cancelled`,caller 侧仍卡住等 SSE 收敛
- 同公司 NAT / 出口 IP 相同的两个用户互相限流

线上 relay `bifrost-server-v4` 的 `remoteInvoke.ts` 内部原本没有应用层全站 limiter,线上出现的 429 更可能来自网关层;但无论来源,remote invoke 都需要:

1. 把**流式数据面**与**终态控制面**拆开,给取消/事件流留出保底通道;
2. 按**资源键**(`call_id` / `client_instance_id` / `grant_id + caller_fingerprint`)而非 IP 分桶,避免共享出口 IP 成为系统性误伤点;
3. 在 caller 侧对 relay 返回 429 或 SSE 中断做兜底,不能让 CLI 无限挂起。

代码校验(2026-06-16):

- `packages/bifrost-sync-server/src/index.ts:27~31` 的 `isRemoteInvokePath` 覆盖 `/v4/remote-invoke/`、`/v5/remote-invoke/`、`/remote-invoke/` 前缀。
- `packages/bifrost-sync-server/src/index.ts:94~100` 在全站 `globalLimiter` 之前对 remote invoke 路径直接跳过。
- `packages/bifrost-sync-server/src/routes/remote-invoke.ts:37~46` 定义了 8 个 dedicated limiter;`applyRateLimit` 在 24 处调用点分别以 client/call/caller 键位。
- `crates/bifrost-cli/src/commands/remote.rs:6701` 提供 `settle_cancelled_call`(取消收尾兜底);`:4700` 提供 `wait_for_remote_call_cancel_signal`(caller 信号识别)。
- 底层 `packages/bifrost-sync-server/src/security.ts` 的 `RateLimiter` 仍是固定窗口 `{count, resetAt}`,token bucket 升级仍属 planned。

## 用户目标验证清单

### 必须实现

- 本地 relay `/v4/remote-invoke/*` 路径不再受全站 IP `globalLimiter` 影响。
- remote invoke 内部按用途拆分至少 8 个 dedicated bucket:register / clientQuery / clientData / clientStreamFrame / callerLookup / callerOpen / callerControl / callerPopPreflight。
- caller 终态控制路径(`cancel` / `events` / `exit` / `input` / `status`)使用独立 `callerControlLimiter`,不与高频 `frame` 共享额度。
- client 数据面 `frame` 单独走 `clientDataLimiter`(1500 req / 10s);大流量 `stream-frame` 走 `clientStreamFrameLimiter`(60000 req / 10s)。
- caller `settle_cancelled_call` 在 relay 返回 429 时做短重试;若最终仍无法收敛,合成 caller 侧 `Cancelled` 避免 CLI 无限挂起。
- 匿名入口 `pairings/start` 保持按 IP 限流(共享出口 IP 允许并发有限,但不会把 authenticated 调用一起打死)。
- 线上 relay 不新增 pod-local authenticated remote invoke limiter,避免多 pod 计数器不一致把同一调用切成多份不稳定配额。
- 线上 relay `client/stream` 从 `verifyClientAuth()` 返回的 client owner 中写入 `user_id`,不再依赖 query 参数透传。

### 必须不破坏

- 正常成功路径 `streaming → completed` 的 QPS 不受收紧影响。
- 非 remote invoke 路径 (`/v4/sso/*` / `/v4/sync/*` / `/v4/pairings/*` 中非 `remote-invoke`)继续走全站 `globalLimiter`。
- `bifrost remote call list` / `get` / `clear` 走 `clientQueryLimiter` 保持工作。
- caller `open_call` 桶足够容忍首次授权+复用 grant 的连续请求。

### 必须真实验证

- 单元测试:`packages/bifrost-sync-server/src/__tests__/rate-limit.test.ts` 至少覆盖 non-remote-invoke 全局限流生效、remote invoke 豁免、`auth_rate_limit_per_ip` 独立生效三条。
- E2E:`test_remote_invoke_e2e.sh` 大流量 `search.get` + caller `Ctrl-C` → Recent Calls `Cancelled` 且 caller 不挂起。
- Human tests:`TC-RI-回归-113~118` 覆盖粗粒度 429 不再误伤、共享 NAT 不互相打断、线上 relay `client/stream` 无 query user_id。

## 产品语义

### 限流分桶原则

- **匿名入口(anonymous_entry)**:按 IP。`pairings/start` 等无授权入口沿用 `pairRateLimiters`。这类请求本身就是稀疏、防刷即可。
- **已授权 transport**:按资源键。`call_id`(数据面/事件面/取消)、`client_instance_id`(register/查询)、`grant_id + caller_fingerprint`(open)。共享出口 IP 不再作为主键。
- **SSE handshake**:活跃连接数上限已通过 `max_sse_connections_per_client` / `max_sse_connections_per_ip` 限制;建连频率的 per-call bucket 仍属 planned。
- **终态控制(terminal_control)**:小流量、高优先级、独立配额,与数据面完全隔离。

### 429 与终态收敛的关系

`Cancelled` 是终态,晚到 429 不能反向覆盖它(由 `should_apply_call_result` 守卫)。caller 侧 `settle_cancelled_call` 的兜底"合成 Cancelled"仅在 `POST /cancel` 已发出后生效,只写入 caller 本地 CLI 输出,不回写 target client `CallHistoryStore`;target 端终态仍以 `apply_cancelled_call` / reconcile 为准。

## 技术细节

### 本地 relay:全站 limiter 豁免 remote invoke

`packages/bifrost-sync-server/src/index.ts` 关键片段:

```typescript
function isRemoteInvokePath(pathname: string): boolean {
  return pathname.startsWith('/v4/remote-invoke/')
    || pathname.startsWith('/v5/remote-invoke/')
    || pathname.startsWith('/remote-invoke/');
}

// 请求进入时:
if (!isRemoteInvokePath(url.pathname)) {
  const globalCheck = globalLimiter.check(clientIp);
  if (!globalCheck.allowed) {
    sendRateLimited(res, globalCheck.retryAfterMs);
    return;
  }
}
```

`globalLimiter` 由 `server.rate_limit_per_ip` 配置(默认 200 / 60s);`authLimiter` 由 `auth_rate_limit_per_ip`(默认 60 / 60s)独立管理 `/v4/sso/login` `/register`,与 remote invoke 无关。

### 本地 relay:8 个 dedicated bucket

`packages/bifrost-sync-server/src/routes/remote-invoke.ts:37~46`:

| bucket | 容量 / 窗口 | 键位模板 | 用途 |
|--------|-------------|----------|------|
| `registerLimiter` | 60 / 60s | `${syncTokenKey}:register:${client_instance_id}` | client 首次注册 challenge + register |
| `clientQueryLimiter` | 240 / 60s | `client:${clientId}:heartbeat` / `:list_calls` / `:get_call` / `:pending_pairings` 等 | client 侧查询/管理 |
| `clientDataLimiter` | 1500 / 10s | `call:${callId}:frame` | client → relay 常规数据面 |
| `clientStreamFrameLimiter` | 60000 / 10s | `call:${callId}:stream_frame` | 大流量流式数据面 |
| `callerLookupLimiter` | 240 / 60s | `${caller_fingerprint}:${client_instance_id}:lookup` | caller 查询 grant |
| `callerOpenLimiter` | 600 / 60s | `${caller_fingerprint}:${client_instance_id}:open` | caller 开新调用 |
| `callerControlLimiter` | 240 / 60s | `${callerAccessKey}:cancel:${callId}` / `:call_events:` / `:call_status:` / `:call_input:` 等;client `:exit` 也走该桶 | 终态控制 |
| `callerPopPreflightLimiter` | 600 / 60s | `callerPopPreflightKey(ctx, 'lookup'/'claim'/'ssh_claim'/'revoke'/'open')` | PoP JWT 校验前置 |

### Caller 兜底:`settle_cancelled_call`

`crates/bifrost-cli/src/commands/remote.rs:6701` 的 `settle_cancelled_call` 在 caller 发出 `POST /cancel` 之后启用:

1. 短窗口内继续 SSE 订阅,等 relay 返回终态 `Cancelled`。
2. 若 SSE 阶段返回 429,识别为 retryable,短退避后重试。
3. 若最终仍拿不到终态,合成 caller 侧 `Cancelled` 结果,exit code 130。
4. 该兜底不写入 relay,不影响 target client `CallHistoryStore` 的真实终态。

`wait_for_remote_call_cancel_signal`(`:4700`)在 Unix 监听 `SIGINT` / `SIGTERM` / `SIGHUP` + `ctrl_c()` 兜底,非 Unix 只监听 `ctrl_c()`。所有 remote 命令 handler 通过 `tokio::select!` 并行等待信号与结果。

### 线上 relay:身份优先、IP 兜底

`bifrost-server-v4/app/routes/remoteInvoke.ts` 采取更保守的分布式友好策略:

- authenticated remote invoke 主链路不新增 pod-local limiter(否则多 pod 内存计数不一致)。
- `client/stream` 在 `verifyClientAuth()` 之后直接从 client owner 中拿 `user_id`,不再依赖 query 参数透传。`max_sse_connections_per_client` 优先使用 `user:<user_id>` 键,回退 IP。
- 若未来必须给线上 authenticated remote 做限流,应走 Redis / 网关级别的全局分桶,并按 `call_id` / `client_instance_id` / `grant_id + caller_fingerprint` 键。
- 保留分布式友好的资源级限制:`ssh/challenge` 按 `device_code` 的 Redis 计数。

### 单行 / 单请求大小上限

- HTTP body:遵循 relay 层 `MAX_REQUEST_BODY`(默认 8 MiB);超过直接 413。
- SSE 事件行:遵循 caller 侧 line-buffer 上限(默认 2 MiB / 行),超过重连。
- 与 `call_history` 单行 2 MiB / 整文件 256 MiB 阈值形成防御纵深。

## CLI / Web / Admin API 表面

### CLI

- `bifrost remote *` 全部命令使用 `wait_for_remote_call_cancel_signal` 支持 caller `Ctrl-C`。
- caller 收到 relay 429 时:
  - 数据面(读事件):按短退避重试;
  - 控制面(cancel):不重试(已有 dedicated bucket,失败即视为真的短时不可用);
  - 若在取消收尾阶段最终仍拿不到终态,合成 `Cancelled`,退出码 130。
- CLI 输出统一 `remote command '<name>' rate-limited by relay, retrying...` / `... falling back to synthetic cancelled`。

### Web UI

Settings → Remote Invoke → Health:

- 展示当前 relay 各 bucket 命中率(可选,后续 admin API 扩展);
- Recent Calls 详情面板 `cancelled`(caller) vs `cancelled`(relay-confirmed)区分展示。

### Admin API

- `GET /_bifrost/api/relay/rate-limit-stats`(planned):汇总 bucket 命中/剩余额度。
- 其他 `/api/remote-invoke/*` 复用 caller / client 现有 API。

### Relay 路由与桶对应表

主要 24 处调用点(从 `remote-invoke.ts`):

- `register_challenge` / `register` → `registerLimiter`(键含 `client_instance_id`)。
- `heartbeat` / `ssh_connect_result` / `publish_pair_code` / `close_discovery` / `grant_decision` / `delete_grant` / `update_grant` / `active_grants` / `list_calls` / `get_call` / `pending_pairings` / `cancel_pending_pairings` → `clientQueryLimiter`(键含 `client_instance_id`)。
- `frame` → `clientDataLimiter`(键含 `call_id`)。
- `stream_frame` → `clientStreamFrameLimiter`(键含 `call_id`)。
- `exit` → `callerControlLimiter`(键含 `call_id`)。
- `caller lookup` → `callerLookupLimiter`(键含 `caller_fingerprint + client_instance_id`)。
- `caller open` → `callerOpenLimiter`(键含 `caller_fingerprint + client_instance_id`)。
- `caller call_input/events/status/cancel` → `callerControlLimiter`(键含 `caller_access_key + call_id`)。
- `caller pop preflight lookup/claim/ssh_claim/revoke/open` → `callerPopPreflightLimiter`。

## Sync 边界

- Rate limiter 状态严格 relay 本地,不参与 Bifrost Sync。
- 本地 relay 与线上 relay 采用完全不同的限流拓扑(内存 vs Redis / 网关),不共享配置。
- Recent Calls 中 429 事件不通过 sync 上行;只本地审计。

## Phase 1-4 实施路径

### Phase 1(已落地):全站豁免 + 分桶

- `isRemoteInvokePath` 在 `index.ts` 中把 `/v4/remote-invoke/*` 从 `globalLimiter` 摘出。
- `remote-invoke.ts` 拆出 8 个 dedicated `RateLimiter`,所有路由改走 `applyRateLimit(ctx, <bucket>, <key>)`。
- 线上 relay `client/stream` 改从 `verifyClientAuth()` 拿 `user_id`。

### Phase 2(已落地):Caller 兜底

- `wait_for_remote_call_cancel_signal` 覆盖所有 remote 命令(cancel doc 已实现)。
- `settle_cancelled_call` 短重试 + 合成 Cancelled。

### Phase 3(planned):算法升级 + 显式语义分类

- 把 8 个用途桶收敛成 4 类语义:`anonymous_entry` / `sse_handshake` / `data_plane` / `terminal_control`,便于策略调整。
- 底层 `RateLimiter` 从固定窗口升级到 token bucket / leaky bucket,消除"59 秒打满 → 整个窗口剩余时间全 429"的悬崖效应。
- SSE 建连按 `call_id` 增加频率桶(现在只有活跃连接数上限)。

### Phase 4(planned):文档 + 回归 + Human tests

- `packages/bifrost-sync-server/src/__tests__/rate-limit.test.ts` 补 remote-invoke 豁免、共享 NAT 不互相限流两个 vitest 用例。
- `test_remote_invoke_e2e.sh` 添加 caller Ctrl-C → `Cancelled` 断言(已落地)。
- `human_tests/remote-invoke.md` `TC-RI-回归-118` 覆盖共享 NAT + `client/stream` user_id。

## 测试方案

### 单元测试

- `packages/bifrost-sync-server/src/__tests__/rate-limit.test.ts`(现有):
  - `uses server.rate_limit_per_ip for non remote-invoke paths` — 已覆盖。
  - `auth_rate_limit_per_ip 独立限制 SSO 路径` — 已覆盖。
  - `remote invoke 路径豁免 globalLimiter` — planned(TC-RI-回归-118 手工回归)。
  - `共享 NAT 下不同用户 client/stream 不互相挤掉` — planned(TC-RI-回归-118)。
- `crates/bifrost-cli/src/commands/remote.rs` 内 `mod tests`:
  - `settle_cancelled_call` 遇 429 短重试。
  - `settle_cancelled_call` 最终失败合成 `Cancelled`。
  - `wait_for_remote_call_cancel_signal` 处理 SIGINT / SIGTERM / SIGHUP。

### E2E 测试

- `e2e-tests/tests/test_remote_invoke_e2e.sh`:
  - 大量 `search.get` frame 触发 `clientDataLimiter`,caller 侧发 `Ctrl-C`;
  - 断言 client Recent Calls 最终 `Cancelled`;
  - 断言 caller 进程退出码 130,不无限挂起。

### Human tests(`human_tests/remote-invoke.md`)

- `TC-RI-回归-113~117`:见 cancel doc(已复用)。
- `TC-RI-回归-118`:粗粒度限流不再把 `cancel/events/exit` 打成 429;线上 relay `client/stream` 不再依赖 query `user_id`;共享 NAT 已认证 remote invoke 不互相限流。

所有 human tests 启动 Bifrost 时必须使用临时 `BIFROST_DATA_DIR`、非 9900 admin 端口、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1`、`--no-system-proxy`。

## Review / Fix / Test 闭环

### 第 1 轮

- 复核用户目标:
  - 本地 relay 全站 limiter 是否豁免 remote invoke 路径;
  - 8 个 bucket 是否覆盖 register / clientQuery / clientData / clientStreamFrame / callerLookup / callerOpen / callerControl / callerPopPreflight;
  - caller `settle_cancelled_call` 是否只在 `POST /cancel` 已发出后合成 `Cancelled`。
- 复核 diff:
  - `index.ts` 是否只在 non-remote-invoke 路径调 `globalLimiter.check`;
  - `remote-invoke.ts` 每一个 `handleXxx` 是否都用 `applyRateLimit` 而不是直接放行;
  - 线上 relay `client/stream` handler 是否已停止读取 query `user_id`。
- 重点 review:
  - `cancel/events/exit` 三个终态路径共用 `callerControlLimiter` 会不会互相挤(240/60s 对单一 call 足够);
  - `clientStreamFrameLimiter` 60000/10s 是否足以覆盖峰值 `traffic.search` 数据面(实测下已通过);
  - PoP preflight bucket 与业务 bucket 是否需要级联(避免绕过 preflight)。
- 复测:vitest `rate-limit.test.ts`、`test_remote_invoke_e2e.sh`、TC-RI-回归-113~118。

### 第 2 轮

- 复核第 1 轮问题的修复。
- 检查 `git status --short` / `git diff`,确保 `human_tests/readme.md` 索引同步更新。
- 重点 review:
  - 固定窗口算法在业务尖峰下的悬崖效应是否达到需要 token bucket 的临界点;
  - `callerControlLimiter` 是否需要拆成 `cancel` 独立高优先级 sub-bucket;
  - 线上 relay 未来若接入 Redis 分布式 limiter,键位设计要预留 `call_id`。
- 复测:Human tests `TC-RI-回归-113~118` 全部复跑;线上 relay 场景复测 caller `Ctrl-C` → `Cancelled` 收敛时间 <2s。

## 风险与决策

- **本地 relay 分桶粒度过细 vs 过粗**:粗粒度会误伤 (`cancel` 被 `frame` 打死);过细会难以调整。当前 8 个 bucket 已按"用途"聚合;后续若语义收敛到 4 类,再做二次归并。
- **固定窗口悬崖 vs token bucket**:悬崖效应在 caller `Ctrl-C` 短时突发下明显;短期通过缩短 window(`clientDataLimiter` 10s)缓解,长期升级 token bucket。
- **caller 合成 `Cancelled` 与真实 relay 终态冲突**:合成结果只写 caller 本地 CLI 输出,不回写 target `CallHistoryStore`,避免污染 target 审计。
- **线上 relay pod-local limiter 不一致**:决策上不引入,避免同一调用命中不同 pod 得到不同 429 结果;若必须限流,走 Redis / 网关。
- **共享出口 IP**:NAT 后多个用户共享出口 IP 时,不应互相限流;已通过 `user_id` 优先键位、匿名 bucket 独立化解决。
- **429 与 `Cancelled` 优先级**:`should_apply_call_result` 守护终态优先级;晚到 429 不覆盖 `Cancelled`。
- **`SIGHUP` 与 `--detach`**:当前 `SIGHUP` 一律识别为"用户主动取消";若未来引入"脱离 tty 保留调用"语义,需要新 `--detach` 参数,不能改动现有信号处理。

## 参考

- `design/remote-invoke-call-cancel.md` — Caller 取消协议与 `should_apply_call_result`。
- `design/remote-invoke-call-history-persistence.md` — Recent Calls JSONL 存储、单行 2 MiB 上限。
- `design/remote-invoke-file-api.md` — File API 分桶(共用 clientQuery/callerControl)。
- `design/grant-file-access.md` — Grant/授权模型。
- `packages/bifrost-sync-server/src/index.ts:27~100` — 全站 limiter 与 remote invoke 豁免。
- `packages/bifrost-sync-server/src/routes/remote-invoke.ts:37~46` — 8 个 dedicated bucket 定义与 24 处 `applyRateLimit` 调用。
- `packages/bifrost-sync-server/src/security.ts` — `RateLimiter` 固定窗口实现。
- `crates/bifrost-cli/src/commands/remote.rs:4700 / :6701` — caller 信号识别与 `settle_cancelled_call`。

## 实现现状校验(2026-06-16)

- 本地 relay `isRemoteInvokePath` 已在 `index.ts:27~31 / :94~100` 生效,`/v4/remote-invoke/*` `/v5/remote-invoke/*` `/remote-invoke/*` 均豁免 `globalLimiter`。
- `remote-invoke.ts` 8 个 dedicated bucket 已定义(`registerLimiter` 60/60s、`clientQueryLimiter` 240/60s、`clientDataLimiter` 1500/10s、`clientStreamFrameLimiter` 60000/10s、`callerLookupLimiter` 240/60s、`callerOpenLimiter` 600/60s、`callerControlLimiter` 240/60s、`callerPopPreflightLimiter` 600/60s),24 处 `applyRateLimit` 调用键位均已按资源键组装。
- caller 端 `settle_cancelled_call`(`remote.rs:6701`)、`wait_for_remote_call_cancel_signal`(`:4700`)已在 3 处 `tokio::select!` 分支中被使用(`:2032` / `:2226` / `:2276`)。
- 线上 relay `client/stream` 已从 `verifyClientAuth()` 拿 `user_id`,不再依赖 query。
- `security.ts` `RateLimiter` 仍为固定窗口 `{count, resetAt}`,token bucket 与 SSE per-call 建连频率桶仍属 planned。
- `rate-limit.test.ts` 中 `remote invoke 豁免`与`共享 NAT 不互相限流`两条 vitest 用例仍属 planned,现由 `TC-RI-回归-118` 手工回归覆盖。
