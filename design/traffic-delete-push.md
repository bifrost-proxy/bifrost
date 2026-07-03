# Traffic 删除推送与详情清理

## 背景

Bifrost admin 支持前端主动删除 traffic 记录（`DELETE /_bifrost/api/traffic`
带 `{ids: [...]}`）以及一键清空。当一台机器上有多个 admin 会话打开时，
其中一个会话删除 traffic，其他会话必须实时看到列表更新、被删除的详情页
必须给出明确提示，而不是保留一个悬空 UI 或让用户看到过期数据。

本文档描述**当前已实现**的 `traffic_deleted` push 事件、后端 side-effect
清理、前端 store 响应逻辑，并把此前留白的边界（清理失败补偿、活跃连接
保护、批量语义）固化为规范。

## 用户目标验证清单

### 必须实现

- 后端删除 traffic 记录后广播 `traffic_deleted` push 事件，携带被删除的
  `ids: string[]`。
- 广播作用于所有已注册 push client（不看 subscription 过滤，删除是全局
  事件）。
- 后端在广播前完成：`traffic_records` 行删除、body store 关联 body 删除、
  frame store frame 删除、ws payload store payload 删除、overview cache
  失效。
- 前端 `useTrafficStore.handleTrafficDeleted` 从 `recordsMap` / `records` /
  `pendingIds` / `clientCatalog` 中同步移除。
- 前端如果当前详情或 `selectedId` 命中被删列表，清空详情、置
  `detailError = 'Request was deleted'`、`selectedId = undefined`。
- 前端 `serverTotal` 相应减少但不低于 0。

### 必须不破坏

- 活跃连接（socket 未关闭、frame_count 仍在增长）不被 `clear` 误删；
  `clear_traffic_by_ids` 在传入 id 集合前会过滤 `is_active`。
- SSE / frames / WebSocket 详情订阅在记录被删后能通过 EventSource 自然
  关闭（记录不再存在，SSE 流返回错误或空结束）。
- push 顺序：先写 DB → 再关联清理 → 再广播；广播失败不阻塞删除主流程。
- 单条删除、批量删除、clear-all 三条路径统一走 `broadcast_traffic_deleted`。
- 空 `ids` 数组不广播。

### 必须真实验证

- 打开两个 admin 页面，一个 DELETE 一条流量，另一个页面列表自动移除。
- 两个页面选中同一条记录，A 删除后 B 的详情面板应显示 `Request was
  deleted` 且列表不再选中该行。
- `clear-all` 后所有页面列表清空，且详情面板都给出提示。
- 后端 body / frame / ws payload store 的对应文件在删除后被清理，无泄漏。

## 产品语义

### `traffic_deleted` 是全局失效通知

`traffic_deleted` 不区分订阅过滤（host / method / status 等），因为“记录
被删除”是数据存在性变化，所有客户端都必须知道。这一点与 `TrafficInserted`
/ `TrafficUpdated` 走过滤 dispatch 不同，请在实现时保持差异。

### 详情被删的 UX 契约

- 详情面板不能保持空白或悬空 loading。
- 用户看到明确文案 `Request was deleted`，方便理解为什么详情消失。
- `selectedId` 被清空后，用户下一次点击其他行不会残留高亮。

### 活跃连接保护

`clear_traffic_by_ids` 在真正 delete 之前把请求中的 `ids` 与 `is_active`
过滤：活跃的 socket/SSE 连接对应记录不参与 delete；这样长连接不会因为
后端错误清空导致中断。UI clear-all 按钮也遵循这个语义。

## 技术细节

### 关键代码入口

- `crates/bifrost-admin/src/push.rs`
  - `PushMessage::TrafficDeleted(TrafficDeletedData { ids: Vec<String> })`
    枚举变体（`#[serde(rename = "traffic_deleted")]`）。
  - `PushManager::broadcast_traffic_deleted(&self, ids: Vec<String>)`
    (line ~1501)：空数组早退，遍历 `self.clients` 逐个 `send`，失败的
    client 收集后 `unregister_client`。
- `crates/bifrost-admin/src/handlers/traffic.rs`
  - `clear_traffic` 分派 → `clear_traffic_by_ids` / `clear_all_traffic`。
  - `clear_traffic_by_ids` 在删除完成后调用
    `pm.invalidate_overview_cache();`
    `pm.broadcast_traffic_deleted(ids_to_delete.clone());`
- `crates/bifrost-admin/src/query_service.rs`
  - 老 query service 中的 delete 路径同样调用 `broadcast_traffic_deleted`
    （line ~388），保证 legacy 路径也发通知。
- `web/src/services/pushService.ts`
  - `TrafficDeletedData { ids: string[] }`。
  - `case 'traffic_deleted':` dispatch 到 `trafficDeletedHandlers` 集合。
  - `onTrafficDeleted(handler)` 提供订阅 API。
- `web/src/stores/useTrafficStore.ts`
  - `handleTrafficDeleted(ids: string[])` (line ~1291)。
  - 订阅：`pushService.onTrafficDeleted((data) => get().handleTrafficDeleted(data.ids));`
    (line ~979)。

### 后端删除 + 广播流程

```
DELETE /_bifrost/api/traffic
  → clear_traffic
    ├── parse body { ids?: [...] }
    ├── if ids present  → clear_traffic_by_ids
    └── else            → clear_all_traffic
                                 ↓
1) 过滤 is_active            (clear_traffic_by_ids)
2) db_store.delete_by_ids   (traffic_records + traffic_record_details CASCADE)
3) body_store.delete_by_ids (spawn_blocking, warn on error)
4) frame_store.delete_by_ids(spawn_blocking, warn on error)
5) ws_payload_store.delete_by_ids
6) pm.invalidate_overview_cache()
7) pm.broadcast_traffic_deleted(ids_to_delete)
8) return HTTP 200 "{n} traffic records cleared successfully"
```

关键点：

- 关联清理任何一步失败只记录 `tracing::warn!` 不中断广播；DB 已经删除，
  遗留的 body / frame 文件由后台 cleanup / retention 兜底。
- `pm.invalidate_overview_cache()` 在广播前先做，避免其他 client 收到
  `traffic_deleted` 后立刻请求 overview 拿到旧值。
- 广播是 broadcast channel `send`，一个 client backpressure 不影响其他
  client；失败客户端在同一函数内 unregister。

### 前端 handler 关键行为

`handleTrafficDeleted(ids)`：

- `idsSet = new Set(ids)`。
- 从 `recordsMap` / `records` / `pendingIds` / `clientCatalog` 移除对应
  条目，累计 `removedCount`。
- `currentDeleted = currentRecord && idsSet.has(currentRecord.id)`。
- `selectedDeleted = selectedId && idsSet.has(selectedId)`。
- 三个都空时直接 `return {};`，避免无谓的 setState 触发订阅链。
- 有变化时同步更新：
  - `records`、`recordsMap`（filter 后重算）。
  - `pendingIds`、`clientCatalog`。
  - `serverTotal = max(serverTotal - removedCount, 0)`。
  - `boundaries = getBoundaryState(records)` 更新 `oldestSequence` /
    `lastId` / `lastSequence`。
  - 若 `detailRemoved`：
    - `currentRecord = null`
    - `requestBody = null`
    - `responseBody = null`
    - `detailLoading = false`
    - `detailError = 'Request was deleted'`
    - `selectedId = undefined`
  - `filterVersion` +1（用于下游 selector memo 失效）。
  - `recordsMutation = createRecordsMutation({ deletedIds })` 供列表虚拟
    滚动做增量 diff。

### push channel 与 broadcast 语义

- `PushManager` 用 `DashMap<u64, Arc<PushClient>>` 管理注册 client。
- `traffic_deleted` 走 per-client `mpsc::Sender::try_send`，失败即认为
  client 断线，直接 unregister。这里刻意不重试；push 是尽力语义，前端
  重新建立 SSE 连接后会用 `server_sequence` + refetch 补齐。

## CLI + Web + Admin API

### CLI

- `bifrost traffic delete <id-or-seq>`：调用后端 DELETE 接口，触发
  `traffic_deleted` 广播。CLI 输出删除条数。
- `bifrost traffic clear`：清空全部，触发广播携带全部被删除 id。

### Web

- Traffic 列表右键 / 批量删除按钮 → `api.deleteTraffic(ids)` →
  `DELETE /_bifrost/api/traffic { ids }`。
- 空状态“Clear all”按钮 → `DELETE /_bifrost/api/traffic` 空 body → 清空。
- 详情面板处理 `Request was deleted` 文案；面板顶部关闭按钮仍可用，允许
  用户手动 dismiss。

### Admin API

- `DELETE /_bifrost/api/traffic`
  - request body: `{ "ids": ["...", "..."] }` 可选；缺省清空全部。
  - response: `200 {"message": "N traffic records cleared successfully"}`。
  - side effect: 广播 `traffic_deleted { ids }`。
- SSE `/api/notifications/stream` 或 `/api/push/stream` 客户端收到：
  ```json
  {"type": "traffic_deleted", "data": {"ids": ["..."]}}
  ```

## Sync 边界

- Traffic 记录不参与云端 sync；`traffic_deleted` 广播只在本机 admin 客户端
  之间生效。
- 远程 Bifrost（`bifrost remote`）拉取远端 push 时也会转发 `traffic_deleted`
  给本地 CLI/agent，用于远端 traffic 视图的同步失效。

## Phase 1-4

本设计已经落地，无新增 Phase。

### Phase 1（历史，已完成）

- 引入 `PushMessage::TrafficDeleted` + `TrafficDeletedData`。
- `PushManager::broadcast_traffic_deleted` 全局广播。

### Phase 2（历史，已完成）

- `clear_traffic_by_ids` 与 `clear_all_traffic` 在删除后统一广播。
- 关联 body/frame/ws payload store 清理与广播绑定。

### Phase 3（历史，已完成）

- 前端 `useTrafficStore.handleTrafficDeleted` 全量处理 3 类清理：
  列表、当前详情、选中项。
- `detailError = 'Request was deleted'` 文案固定。

### Phase 4（当前维护）

- 保持“空 ids 不广播”“活跃连接过滤”“广播前先失效 overview cache”
  三条不变量。

## 测试方案

### 单元测试

- `crates/bifrost-admin/src/push.rs`
  - `broadcast_traffic_deleted_sends_message`（line ~2940）：验证广播消息
    到达注册 client。
- `crates/bifrost-admin/src/handlers/traffic.rs`
  - `clear_traffic_request_tests::malformed_clear_request_is_rejected`
  - `clear_traffic_request_tests::empty_clear_request_keeps_clear_all_semantics`
  - `clear_traffic_request_tests::ids_clear_request_parses_ids`
- `crates/bifrost-admin/src/traffic_db/store.rs`
  - `test_clear_removes_pending_records_when_no_active_connections`
  - `test_clear_preserves_active_connection_records`
  - `test_maybe_cleanup_record_count_only_deletes_to_target`

建议补齐：

- `broadcast_traffic_deleted_skips_when_ids_empty`
- `broadcast_traffic_deleted_unregisters_dead_clients`

### 前端单元测试

- `web/src/stores/useTrafficStore.test.ts`
  - `onTrafficDeleted` mock 已存在（line 10）。
  - 建议补齐 `handleTrafficDeleted` 三条：
    - 移除普通行 → `records` / `recordsMap` / `serverTotal` 更新。
    - 命中当前详情 → `detailError = 'Request was deleted'` + `currentRecord = null`。
    - 命中 `selectedId` → `selectedId = undefined`。

### E2E

- `.agents/skills/e2e-verify/scripts/scenarios/traffic-delete.json`：
  已有场景 assert `document.body.innerText.includes('Request was deleted')
  || 'not found'`。
- `crates/bifrost-e2e/src/tests/traffic_*`：可扩展一个用例，验证多客户端
  下 A 删 B 收到广播。

### human_tests

- `human_tests/webui-traffic.md`：已有 “删除请求详情提示” 用例，覆盖
  单次删除 + 详情提示。
- 建议新增 `TC-TDP-01 多客户端删除广播`。

## Review / Fix / Test 闭环

### 第 1 轮

- 复核代码路径：`clear_traffic` → `clear_traffic_by_ids` / `clear_all_traffic`
  是否都走 `broadcast_traffic_deleted`；`query_service.rs` 老路径是否遗漏。
- 复核前端订阅：`useTrafficStore.subscribe` 中 `pushService.onTrafficDeleted`
  绑定生命周期是否跟随 store 初始化 + 卸载正确注销。
- 重点 review：`handleTrafficDeleted` 三个 setState 分支的 memo 依赖是否
  一致，避免 diff 后仍触发无意义重渲染。

### 第 2 轮

- 复核 `detailError = 'Request was deleted'` 文案在国际化 pipeline 中是否
  需要 i18n key；第一版建议保持英文常量，避免多语言分歧。
- 复核活跃连接过滤：`is_active` 判定与 socket store 状态是否一致。
- 复核 body/frame/ws payload store 删除失败仅 warn 不阻塞广播的语义是否
  符合产品预期；如果需要 “删除失败回滚”，需要新一轮设计。

## 风险与决策

- **决策**：`traffic_deleted` 不做订阅过滤，所有 client 均广播。原因：
  数据存在性变化，比订阅过滤更重要。
- **决策**：关联清理失败仅 warn 不回滚。原因：DB 已经删除，回滚会造成
  更复杂的一致性问题；孤儿 body/frame 文件由 retention 兜底。
- **风险**：极大批量删除（>10k）时广播消息本身较大。当前实现直接把
  全部 id 塞进一条消息；如果出现性能问题，可以分片广播，但目前无实测
  瓶颈，不做优化。
- **风险**：前端 `detailError` 文案是英文固定串。若产品需要 i18n，需要
  同时改 e2e-verify 断言与 human_tests。
- **风险**：push channel 满时该 client 直接被 unregister，前端需要在
  push 断线后自动重连。当前 `pushService` 已有 auto reconnect + fetch
  补齐路径。
