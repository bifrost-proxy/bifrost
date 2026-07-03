# Web Admin 指标与代理请求降载设计

## 背景

Bifrost Web Admin 早期实现里，`useGlobalDataSync()` 会周期性拉取 metrics history、system proxy、CLI proxy 状态，同时 Traffic 页面无论用户是否打开都在全局层维持 500ms 增量查询。结果：

- 用户打开管理端但停留在 Settings/Rules 等非 Traffic 页面时，浏览器与后端仍在持续发起 metrics/proxy 状态请求，`bifrost` 进程 CPU 出现锯齿。
- Traffic 空闲状态下，后端仍每 500ms 扫库执行 `TrafficDbStore::query() / get_by_ids()`，把 SQLite IO 压在热点上。

本次优化把 Web 端“高频轮询 + 全局订阅”改成 **启动一次快照 + 页面按需 + push 增量 + 空闲短路** 的组合模型。目标是降低 CPU、SQLite 磁盘 IO 和网络握手噪声，同时保留实时体验。

## 用户目标验证清单

### 必须实现

- Web Admin 启动阶段（`useGlobalDataSync()`）只对 `systemProxy`、`cliProxy`、`overview` 各拉取一次；不再做周期轮询。
- Metrics 实时数据通过 `pushService` 订阅 `needOverview + needMetrics` 维持；不再靠前端定时器补偿。
- Metrics history 从全局启动拉取改为 `Settings` 页面切到 `metrics` tab 时才触发 `fetchHistory(3600)`。
- Traffic 实时订阅只在挂载 `web/src/pages/Traffic/index.tsx` 期间启动，卸载时立即停止。
- WebSocket 订阅协议新增 `need_traffic` 字段，后端只对显式 traffic 订阅者执行 `broadcast_traffic_delta`。
- 后端 `crates/bifrost-admin/src/push.rs` 的 traffic 周期广播加入空闲短路：如果客户端 `pending_ids` 为空且 `TrafficDbStore::current_sequence()` 未推进，则本轮跳过 SQLite 查询。

### 必须不破坏

- Traffic 页面挂载期间的增量体验一致：新请求会实时上屏，长连接状态会实时更新。
- Metrics 页面用户体验一致：实时曲线通过 push 通道更新，历史 tab 一次性拉取。
- systemProxy / cliProxy / overview 显示不因移除周期轮询而卡死（管理端已经很少变动这些数据）。
- push 通道其它订阅字段（`need_values`、`need_overview`、`need_metrics`）语义不变。
- 后端 admin API HTTP 面不变。

### 必须真实验证

- 打开管理端后停留在 Settings/Rules 页面：WebSocket 订阅报文里 `need_traffic = false`；进程无 traffic 相关 SQLite 查询。
- 打开 Traffic 页面：WebSocket 订阅报文里 `need_traffic = true`；有新请求时能实时上屏；无活跃流量时 CPU 与 SQLite 查询显著下降。
- Settings → Metrics tab：切入时触发一次 `fetchHistory(3600)`；切出后不再周期请求。

## 产品语义

### 三种数据的三种同步策略

- 一次性快照：`systemProxy`、`cliProxy`、`overview`（首次进 Web Admin 时各拉一次）。
- push 增量：`overview`、`metrics`、`traffic`、`values`（挂载相应页面时开启订阅）。
- 页面按需拉取：`metrics history`（Settings→metrics tab 触发 `fetchHistory(3600)`）。

### 全局订阅字段

`useGlobalDataSync()` 只维持：

- 全局默认启用：`metrics.enablePush({ needOverview: true, needMetrics: true })`（第一版为了让顶部状态条常显）。
- 全局默认关闭：`traffic.enablePush()` —— 该订阅由 Traffic 页面自行开关。

### 后端 traffic 空闲短路

`crates/bifrost-admin/src/push.rs` 的 `broadcast_traffic_delta()`：

- 遍历 client subscriptions，跳过 `need_traffic = false` 的客户端。
- 若客户端 `pending_ids` 为空且 `current_sequence` 未推进，跳过 SQLite 查询，直接进入下一 tick。
- 保证后端在管理端页面停留在非 Traffic 页时不做磁盘 IO。

## 技术细节

### 前端关键源码

- `web/src/hooks/useGlobalDataSync.ts`
  - 启动一次快照：`proxyStore.fetchSystemProxy()`、`proxyStore.fetchCliProxy()`、`metricsStore.fetchOverview()`。
  - `useMetricsStore.getState().enablePush({ needOverview: true, needMetrics: true })` 维持顶部状态条。
  - Traffic 相关订阅在此关闭；仅 catch-up 与 disable 逻辑。
- `web/src/stores/useMetricsStore.ts`
  - 提供 `enablePush({ needOverview, needMetrics })`、`disablePush()`。
- `web/src/stores/useTrafficStore.ts`
  - 提供 `enablePush()`、`disablePush()`、`startPolling()`（历史兼容名，实际走 push）、`stopPolling()`、`catchUpUpdates()`。
- `web/src/pages/Traffic/index.tsx`
  - 挂载时 `useTrafficStore.getState().enablePush()`；卸载时 `disablePush()` + `stopPolling()`。
- Settings/metrics tab
  - 首次切入时触发 `fetchHistory(3600)`。

### 后端关键源码

- `crates/bifrost-admin/src/push.rs`
  - `ClientSubscription` 新增 / 保留 `need_traffic`, `need_overview`, `need_metrics`, `need_values` 字段。
  - `broadcast_traffic_delta()` 空闲短路。
  - `update_pending_ids()` 计算 pending，配合 `current_sequence` 判断是否需要广播。
- `crates/bifrost-admin/src/handlers/websocket.rs`
  - handshake 支持所有订阅字段。
- `crates/bifrost-admin/src/traffic_db.rs`（推断路径）
  - `TrafficDbStore::current_sequence()` 供 push 层判断增量点。

### CLI + Web + Admin API

- CLI：无改动。
- Web：改动集中在 `useGlobalDataSync.ts`、`useMetricsStore.ts`、`useTrafficStore.ts`、`Traffic/index.tsx`、`Settings/metrics` tab。
- Admin API：WebSocket 订阅协议新增 `need_traffic` 字段；HTTP endpoint 保持稳定。

## Sync 边界

- 本次优化仅影响 Web ↔ Admin 之间的实时数据传输策略，不参与 rule/value 的多设备 sync。
- 不影响 CLI 的 `bifrost tail`、`bifrost log`、`bifrost sync` 等命令。
- 不改变 admin push 通道对 `values_update`、`notification`、`metrics` 等既有消息的语义。

## 实现切分

### Phase 1：全局轮询收敛（已上线）

- 移除 `useGlobalDataSync()` 中的定时器。
- 保留启动一次的 `fetchSystemProxy` / `fetchCliProxy` / `fetchOverview`。
- Metrics history 挪到 Settings→metrics tab。

### Phase 2：Traffic 页面订阅隔离（已上线）

- Traffic 页面挂载时开启 `enablePush`，卸载时 `disablePush` + `stopPolling`。
- WebSocket 订阅字段增加 `need_traffic`。

### Phase 3：后端空闲短路（已上线）

- `broadcast_traffic_delta()` 加入 `pending_ids` + `current_sequence` 检查。
- `push.rs` 保证对无 `need_traffic` 的客户端不做 SQLite 查询。

### Phase 4：文档现代化（本次）

- 更新本文件为完整设计，覆盖 Sync 边界与 Review/Fix/Test 闭环。
- 补齐 human_tests 与 E2E 索引。

## 测试方案

### 单元测试

- 前端：`pnpm --dir web exec vitest run src/hooks/useGlobalDataSync`、`src/stores/useTrafficStore`、`src/stores/useMetricsStore`。
- 后端：`cargo test -p bifrost-admin push` 覆盖 `broadcast_traffic_delta`、`update_pending_ids` 相关。

### E2E 测试

- Playwright（涉及页面）：
  - `web/tests/ui/traffic-*.spec.ts` 中的既有 traffic 用例
  - `web/tests/ui/settings-*.spec.ts` 中 metrics tab 相关用例
- 后端脚本（间接验证 push 通道）：`e2e-tests/tests/` 目录下 traffic/metric 相关脚本。
- 相关文档指纹：`human_tests/api-push.md`。

### 真实场景测试 human_tests

- `human_tests/api-push.md`
  - `TC-METRICS-H01`：停留 Settings 时 WebSocket 订阅报文包含 `need_traffic = false`。
  - `TC-METRICS-H02`：进入 Traffic 页面后订阅切换为 `need_traffic = true`，新请求实时上屏。
  - `TC-METRICS-H03`：空闲 60 秒内 `broadcast_traffic_delta` 不触发 SQLite 查询（通过日志 / 性能采样验证）。
  - `TC-METRICS-H04`：Settings → metrics tab 切入触发一次 `fetchHistory(3600)`。
- 同步更新 `human_tests/readme.md` 索引。

### 覆盖率与项目校验

- `pnpm --dir web exec tsc --noEmit`
- `pnpm --dir web exec vitest run`（按 glob）
- `pnpm --dir web exec playwright test tests/ui`（按 tag/glob）
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test -p bifrost-admin push`
- `cargo test --workspace --all-features`（若资源允许）
- `bash scripts/ci/local-ci.sh --skip-e2e`
- 按 `rust-project-validate` 要求执行。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核用户目标：启动一次快照、Metrics push 常驻、Traffic 页面隔离订阅、后端空闲短路。
- 重点 review：
  - `useGlobalDataSync.ts` 里所有 `fetchXxx()` 是否只出现一次调用。
  - Traffic 页面 unmount 是否可靠触发 `disablePush()` + `stopPolling()`（受 StrictMode 双执行影响需注意）。
  - 后端 `broadcast_traffic_delta` 是否在 `pending_ids` 空且 `current_sequence` 未推进时真的跳过 SQLite。
  - `need_traffic` 序列化字段命名与前端保持一致。
- 复跑受影响 vitest / Playwright / cargo test。

### 第 2 轮

- 复审第 1 轮修复后的最新 diff。
- 重点 review：
  - 多标签页并发时 `need_traffic` 计数是否正确（后端应基于全部客户端合并判断）。
  - `catchUpUpdates()` 在页面重新可见后是否补齐掉线期间的 traffic。
  - 顶部状态栏 `overview` push 不受本次改动影响。
- 复跑受影响测试；若仍有回归追加第 3 轮直到关闭。

## 风险与决策点

- **overview push 全局常驻**：为了顶部状态栏体验，我们保留 `needOverview + needMetrics` 全局默认订阅。若未来这两者也变成大数据量，可以进一步下推到显示状态栏的组件挂载周期。
- **StrictMode 双执行**：React 18 StrictMode 下 mount 会跑两次；`enablePush/disablePush` 需要幂等。当前实现依赖 store 引用计数或 unsubscribe 幂等，若发现日志里 push 频繁开关，需要复核。
- **断线补偿**：Traffic 长时间断线后可能错过若干 `traffic_delta`；`catchUpUpdates()` 是唯一补偿路径。若后端不再返回 catch-up 数据，需要设计 pagination 补偿机制。
- **持续观测**：本机 profiling 显示 CPU 锯齿明显下降，热点不再落在 `broadcast_traffic_delta -> TrafficDbStore::query/get_by_ids`。生产环境建议持续通过 `bifrost status --format json` + admin metrics 观察。
