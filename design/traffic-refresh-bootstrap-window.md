# Traffic 刷新首屏窗口稳定性

## 背景

Bifrost WebUI Traffic 页在“刷新页面”或“首次进入”时，历史实现只会拉到一小段 tail，然后再等实时 Push delta 补充。造成三个体验问题：

1. 数据库中已有数万条流量，但用户看不到首屏之外的历史；即使切到 `Only errors` 也感觉“流量丢了”。
2. 页面刷新期间发生的新流量与首批补数可能顺序错乱、跳号。
3. 浏览器窗口 hidden 后恢复、或 WebSocket 重连时，用户会漏掉窗口期新增。

同时 store 侧因 `records` 数量放大到万级，`clientApps / clientIps / domains` 派生每次 `records` 变化都全表 rebuild 会导致主线程卡顿。

本方案统一以下能力：

- 首屏窗口固定“最新 500 条”（HTTP `updates` 与 Push `send_initial_traffic` 两路对齐）。
- 前端维护 `lastSequence` / `oldestSequence` 两条边界游标：
  - `lastSequence` → forward 拉实时增量。
  - `oldestSequence` → backward 后台历史回填直至封底。
- 历史回填独立 GET 分页 + retry / backoff，不阻塞主流。
- 前端列表按 sequence 升序不变量、线性归并、原位替换。
- 窗口 hidden 恢复 / WebSocket 重连触发一次 catch-up。
- 派生 `clientApps / clientIps / domains` 三张 map 改为增量维护，不再遍历 `records` 重建。

> 实现状态（2026-07-03 复核）：
> - Admin API `handlers/traffic.rs` 已实现 `after_id / after_seq / direction=backward / cursor / limit` 解析（`:1050-1080`）与 `updates` 空游标时 `query_latest_window(500)`（`:989, 2188`）。
> - Push 层 `push.rs:846 send_initial_traffic → send_initial_traffic_delta`（`:794-813`）在 `subscription.last_sequence` 为空时同样调用 `query_latest_window(500)`；`last_sequence` 单调递增靠 `.max()` 保证（`:394-400, 667`）。
> - 前端 `web/src/stores/useTrafficStore.ts` 已实现 `lastSequence / oldestSequence`（`:29, 27`），`fetchInitialData`（`:1353`）触发 `backfillHistory`（`:1411, 1420`），实时 `fetchUpdates`（`:1548`），Push visibilitychange / reconnect 回补（`:1670-1853`）。
> - `useGlobalDataSync.ts` 负责与 push 服务对接；三张计数 map 已按“增量维护”实现（详见 `traffic-filter-panel-counts` 设计）。

## 用户目标验证清单

### 必须实现

- Traffic 页首屏或刷新后立刻加载“最新 500 条”，用户能看到最近流量而不是空列表。
- 后台自动 backward backfill 直到数据库最老一条被拉回；用户可以看到完整历史。
- 实时 push delta 保持单调 forward，`last_sequence` 不回退。
- 窗口 hidden 恢复或 WS 重连时主动 catch-up，弥补 backlog。
- 前端列表始终按 `sequence` 升序，不做全表重排。
- `clientApps / clientIps / domains` 计数增量维护，不因 `records` 变化触发整表重算。

### 必须不破坏

- 现有 filter / search / detail / push / 导入导出行为不变。
- 单条状态更新（例如 pending → completed）原位替换，不改变位置。
- 页面拖动、Filter Panel 折叠、Toolbar 交互不受影响。
- Push 与 HTTP 补数路径共用 `query_latest_window(500)`，避免两路各拿一批不同数据。

### 必须真实验证

- Rust 单测：`query_latest_window(N)` 返回最新 N 条且顺序升序。
- Push 单测：`last_sequence` 为空时初始 push 返回最新窗口而非最老窗口。
- Web UI E2E：构造超过 500 条的大批量流量，且只让最新几条命中特定筛选条件；首次进入 + 刷新后仍能看到这些记录。
- Web UI E2E：构造只在更老历史页存在的记录，页面启动后自动 backfill 并最终显示。
- Web UI E2E：窗口 hidden 后恢复 → 显式 catch-up，不漏 backlog。

## 产品语义

### 首屏 vs 增量 vs 回填

三种数据源共享 sequence 单调升序：

| 来源 | 目标 | 游标 |
| --- | --- | --- |
| 首屏窗口 | 最新 500 条 | 无 |
| 实时 forward 增量 | 从 `lastSequence` 之后 | `after_seq=lastSequence` |
| 后台 backward 回填 | 从 `oldestSequence` 向更老 | `direction=backward&cursor=oldestSequence` |

三路合并后仍是同一条按 sequence 升序排列的列表。

### 顺序不变量与归并

- 历史批次先转成升序，然后与当前数组做“前插 / 线性归并”，不做全表 sort。
- 实时增量批次做“后插 / 线性归并”。
- 状态更新的记录（例如响应完成）原位替换，不改变位置。
- `records.length` 达到内存上限（例如 20 000）时，从最老侧丢弃并同步更新 `oldestSequence`。

### Push 与 HTTP 共用最新窗口语义

- `send_initial_traffic` 与 HTTP `/traffic/updates`（无 cursor）都调用 `db_store.query_latest_window(500)`，返回一致数据。
- 避免“HTTP 首屏拿一批，Push 首屏又拿另一批”造成漂移。

### push `last_sequence` 单调递增

- Push 客户端订阅内 `subscription.last_sequence` 用 `.max()` 更新，避免首次补数与实时 delta 交错时游标回退（`push.rs:394-400, 667`）。
- 每次 push delta 完成后 `sub.last_sequence = Some(sub.last_sequence.map_or(seq, |c| c.max(seq)))`。

## 技术细节

### Admin API：`/traffic/updates`

`crates/bifrost-admin/src/handlers/traffic.rs`：

```rust
// updates 参数解析（:1050+）
struct UpdatesParams {
    after_id: Option<String>,
    after_seq: Option<u64>,
    direction: Direction,   // forward | backward
    limit: Option<usize>,
    pending_ids: HashSet<String>,
}

// 处理路径
match (params.after_id, params.after_seq) {
    (None, None) => db_clone.query_latest_window(500),      // 首屏 tail
    _ => db_clone.query_records(params),                    // 增量或回填
}
```

- `direction=forward`：实时增量（`after_seq` 之后到最新）。
- `direction=backward`：历史回填（从 `cursor` 向更老，limit 500）。
- 空游标：`query_latest_window(500)`。

### Admin API：`/traffic`（分页历史）

```
GET /_bifrost/api/traffic?direction=backward&cursor=<oldestSequence>&limit=500
```

- 用于前端 `backfillHistory` 分页拉取更老数据。
- 单页失败不永久停止，前端按 backoff 重试。

### Push：初始窗口

`crates/bifrost-admin/src/push.rs`：

```rust
fn send_initial_traffic_delta(&self, client, db_store, subscription) {
    let result = if let Some(cursor) = subscription.last_sequence {
        db_store.query_records(TrafficQuery {
            after_seq: Some(cursor),
            direction: Direction::Forward,
            limit: Some(500),
            ..
        })
    } else {
        db_store.query_latest_window(500)   // 与 HTTP 首屏对齐
    };
    // ...
}

pub fn send_initial_traffic(&self, client: &Arc<PushClient>) {
    // 首次订阅或重连时触发 send_initial_traffic_delta
}
```

单调递增：

```rust
sub.last_sequence = Some(sub.last_sequence.map_or(seq, |c| c.max(seq)));
```

### 前端：两条游标 + 三条 fetch 路径

`web/src/stores/useTrafficStore.ts`：

```ts
type State = {
  records: TrafficSummary[];
  lastSequence: number | null;    // 最新已加载
  oldestSequence: number | null;  // 最老已加载
  clientAppCounts: Map<string, number>;
  clientIpCounts: Map<string, number>;
  domainCounts: Map<string, number>;
  // ...
};

fetchInitialData: async () => {
  const batch = await api.getUpdates({ limit: 500 });   // 空游标 → tail
  mergeAscending(batch);
  updateSequences(batch);
  void get().backfillHistory();                         // 后台开启回填
};

fetchUpdates: async () => {
  const batch = await api.getUpdates({ after_seq: get().lastSequence });
  mergeAscendingTail(batch);
  updateLastSequence(batch);
};

backfillHistory: async () => {
  while (get().oldestSequence != null) {
    try {
      const batch = await api.get(`/traffic?direction=backward&cursor=${get().oldestSequence}&limit=500`);
      if (batch.length === 0) break;
      mergeAscendingHead(batch);
      updateOldestSequence(batch);
    } catch {
      await backoffSleep();
      continue;   // retry
    }
  }
};
```

### 前端：窗口恢复 / 重连 catch-up

- `document.visibilitychange` 从 `hidden` 变 `visible` 时触发 `fetchUpdates()`。
- Push 服务 (`web/src/services/pushService.ts`) 重连成功后触发 `fetchUpdates()`。
- 不完全依赖 push 首批补数，避免 reconnect 窗口漏 backlog。

### 前端：三张计数 map 增量维护

- 详见 `design/traffic-filter-panel-counts.md`。
- 所有 mutation 路径（`fetchInitialData` / `backfillHistory` / `fetchUpdates` / `handleTrafficPush` / `handleTrafficDelta` / `handleTrafficDeleted` / `clearTraffic`）都必须 increment / decrement 三张 map。
- 禁止在 render / `useMemo` 中对 `records` 重扫。

### 前端：客户端筛选增量维护

- 客户端筛选结果基于 mutation 做增量插入、替换、删除；只在筛选条件变化或全量 reset 时重算。
- 避免每次 records 变化都重跑全部 filter compile。

## CLI + Web + Admin API

### Admin API

- `GET /_bifrost/api/traffic/updates?[after_id=&after_seq=&direction=forward|backward&limit=&pending_ids=]`
- `GET /_bifrost/api/traffic?direction=backward&cursor=<seq>&limit=<n>`
- Push：`send_initial_traffic` 复用 `query_latest_window(500)`。

### CLI

不新增命令。既有：

- `bifrost traffic list --limit 500`
- `bifrost traffic get <id>`

调试首屏窗口时可通过 CLI 或 curl 验证：

```bash
curl "http://<host>:<port>/_bifrost/api/traffic/updates?limit=500"
curl "http://<host>:<port>/_bifrost/api/traffic?direction=backward&cursor=42&limit=500"
```

### Web

- Traffic 页首屏 / 刷新自动加载 tail，后台 backfill。
- 窗口 hidden 恢复 / WS 重连自动 catch-up。
- 顺序始终升序，状态更新原位替换。

## Sync 边界

- 本能力只影响 traffic 数据加载路径，不涉及规则、Group、设备同步。
- Push 与 HTTP 共用同一 store 与顺序不变量，不会造成两个页面之间不一致。
- 多标签页各自维护自己的游标；每个页面独立完成首屏 + backfill + catch-up。

## Phase 1：Admin API 与 Push 对齐

- `/traffic/updates` 空游标时 `query_latest_window(500)`。
- `/traffic` 支持 `direction=backward&cursor=&limit=`。
- Push `send_initial_traffic` 空 `last_sequence` 复用同一窗口查询。
- Push `last_sequence` 单调 `.max()`。

## Phase 2：前端游标与增量

- Store 维护 `lastSequence` / `oldestSequence`。
- `fetchInitialData` / `fetchUpdates` / `backfillHistory` 三路。
- 顺序不变量 + 前插 / 后插 / 线性归并。
- 状态更新原位替换。

## Phase 3：可靠性与恢复

- `backfillHistory` 单页失败 retry + backoff。
- `visibilitychange` 与 Push 重连触发 catch-up。
- 达到 records 上限时从最老侧丢弃。

## Phase 4：派生开销降级

- 三张计数 map 增量维护。
- 客户端筛选结果增量维护，仅条件变化或 reset 时重算。
- 更新 human_tests。

## 测试方案

### 单元测试

- `bifrost-admin::traffic_db::query::query_latest_window_returns_newest_ascending` — 返回最新 N 条并按 sequence 升序。
- `bifrost-admin::handlers::traffic::updates_empty_cursor_returns_latest_window` — `/traffic/updates` 空游标走 `query_latest_window(500)`。
- `bifrost-admin::handlers::traffic::updates_parses_direction_and_cursor` — 已存在 `:205, :225, :293` 覆盖 `after_id / after_seq / direction / cursor / limit` 解析。
- `bifrost-admin::push::send_initial_traffic_uses_latest_window_when_last_sequence_empty` — 覆盖 `push.rs:813`。
- `bifrost-admin::push::last_sequence_is_monotonic` — 覆盖 `.max()` 逻辑。

### E2E

- `e2e-tests/tests/test_traffic_persistence_e2e.sh`：验证首屏 / backfill / catch-up 路径的持久化行为。
- `e2e-tests/tests/test_traffic_push_e2e.sh`：Push 首次订阅返回最新窗口；断线重连后 `last_sequence` 单调递增。
- `e2e-tests/tests/test_traffic_db_e2e.sh`：`/traffic?direction=backward&cursor=&limit=` 分页正确。

### Web UI E2E（Playwright）

`web/tests/ui/traffic.spec.ts` 追加：

- `first-paint shows latest 500` — 构造 >500 条流量，刷新后立刻看到最近记录。
- `background backfill loads older records` — 只在老页出现的记录最终自动显示。
- `visibility-restore triggers catch-up` — 页面 hidden → 期间新增 → 恢复后立刻可见。
- `filter selection survives backfill` — backfill 期间已选 filter 不被丢失。
- `sequence order preserved after backfill` — 断言列表 sequence 升序。

### 人工验证 human_tests

- `human_tests/webui-traffic.md`：TC-TRB-01 首屏 + backfill + catch-up；TC-TRB-02 长时间 hidden 恢复。
- `human_tests/api-traffic.md`：`GET /traffic/updates?limit=500` 与 `GET /traffic?direction=backward` 的手工验证。

## Review / Fix / Test 闭环

### 第 1 轮

- 复核用户目标：首屏 tail、backfill 封底、catch-up、顺序不变量、派生增量。
- 复核 diff：Admin API 参数解析、Push 初始窗口、store 三路 fetch、visibility handler。
- 复核语义：`query_latest_window(500)` 一处定义，两路调用；`last_sequence` 单调 `.max()`。
- 复测：Rust 单元 + Push 单元 + Playwright + shell E2E。

### 第 2 轮

- 复核第 1 轮修复：backfill 失败 retry / backoff 是否会永久卡死；catch-up 是否会重复插入。
- 检查边界：records 达到上限的截断、多标签页游标独立、Push 长时间断开后重连补齐。
- 复测：完整 E2E + Playwright + human_tests 复跑。

## 风险与决策

- **决策**：首屏窗口固定 500 条，不做用户可配置，先保证 UX 一致；后续可根据用户量做梯度。
- **决策**：Push 与 HTTP 首屏都走 `query_latest_window(500)`，杜绝两路数据源漂移。
- **风险**：老库存量大时 backfill 长时间运行占带宽 / CPU。缓解：单页 500 条 + 指数退避 + 用户主动清空 / reset 会中断 backfill。
- **风险**：状态更新原位替换要求 `records` 定位 O(1)；缓解：store 内维护 `idIndex` map。
- **风险**：`last_sequence` 若未走 `.max()` 更新，reconnect 时可能回退导致遗漏。缓解：`push.rs` 强制 `.max()`，并加单元测试。
- **风险**：`clientApps / clientIps / domains` 若在渲染层重扫会与首屏 tail 冲突。缓解：增量维护统一由 store mutation 触发。

## 依赖文件

- `crates/bifrost-admin/src/handlers/traffic.rs`
- `crates/bifrost-admin/src/traffic_db/store.rs`
- `crates/bifrost-admin/src/traffic_db/query.rs`
- `crates/bifrost-admin/src/push.rs`
- `web/src/api/traffic.ts`
- `web/src/stores/useTrafficStore.ts`
- `web/src/hooks/useGlobalDataSync.ts`
- `web/src/services/pushService.ts`
- `web/src/types/index.ts`
- `web/tests/ui/traffic.spec.ts`
- `e2e-tests/tests/test_traffic_persistence_e2e.sh`
- `e2e-tests/tests/test_traffic_push_e2e.sh`
- `e2e-tests/tests/test_traffic_db_e2e.sh`
- `human_tests/webui-traffic.md`
- `human_tests/api-traffic.md`

## 文档更新要求

- 本次为行为修复，不涉及 README / API 公共文档变更。
- 更新 `human_tests/webui-traffic.md` 与 `human_tests/api-traffic.md`，加入首屏 / backfill / catch-up 用例。
- 若首屏窗口大小或 backfill 步长改动，同步更新本文件。
