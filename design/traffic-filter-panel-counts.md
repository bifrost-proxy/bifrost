# Traffic Filter Panel Counts 设计方案

## 背景

Bifrost WebUI 的 Network / Traffic 页面左侧提供 `Filter Panel`（`web/src/components/FilterPanel/index.tsx`），列出当前已加载流量按 Client IP、Applications、Domains 三个维度的候选值。用户在展开候选时需要判断“哪一个来源更值得展开筛选”，如果只显示裸候选名，用户必须先勾选筛选项、再看主表长度才知道数量，效率低且反直觉。

为解决这个问题，`FilterPanel` 与 `PinnedFilters` 已经接入 store 中现成的三张实时计数表（`clientIpCounts` / `clientAppCounts` / `domainCounts`），在每个候选行右侧展示计数徽标（例如 `api.example.com  315`），让用户在不点击的前提下感知流量分布。

本方案覆盖当前实现约束与后续扩展路径：

- 第一层是“当前已加载 records 窗口内的裸计数”，不做 faceted contextual counts。
- 第二层是可选的“基于其他 facet 条件的动态计数”，出于成本考虑第一版不做，只留扩展位。

## 用户目标验证清单

### 必须实现

- `FilterPanel` 的 Client IP / Applications / Domains 三段候选列表每项右侧都展示一个计数徽标。
- `PinnedFilters` 展示的固定项也带同一计数，保证固定项与列表候选行为一致。
- 计数口径固定为 “当前前端 store `records` 中该值出现的条数”。
- 计数随实时增量、Push delta、导入、删除、清空等操作即时同步。
- 计数不因 toolbar filter / add filter / 面板搜索关键字变化而重算。
- 面板搜索时展示原始 count，不叠加“命中关键字后剩余数量”。
- 空 store 时候选列表为空，计数徽标不出现。

### 必须不破坏

- Filter Panel 折叠 / 拖拽宽度 / 三段候选滚动布局不受影响。
- 选择候选后主表筛选行为不变。
- 计数徽标不参与主表虚拟列表滚动性能。
- 三张 count map 的维护路径（`fetchInitialData` / `backfillHistory` / `fetchUpdates` / `handleTrafficPush` / `handleTrafficDelta` / `handleTrafficDeleted` / `clearTraffic`）不能被绕过。
- 首屏窗口 / 历史回填 / 实时增量三种来源都必须调用同一段增量维护，禁止在渲染层重新用 `records.filter(...).length` 计算。

### 必须真实验证

- WebUI E2E 打开 Traffic 页，构造 ≥3 组不同 client_ip / app / host，断言 Filter Panel 每项都展示与 store 中一致的计数。
- 删除单条记录 / 清空全部流量后，断言计数徽标同步变化或消失。
- Pinned 项和普通列表项在同一 client_ip 下计数完全一致。

## 产品语义

### 什么是 “候选计数”

候选计数 = 当前浏览器已经拉到并保留在 `useTrafficStore.records` 里的 traffic summary 中，该字段值出现的条数。

它是“候选值分布”，不是“最终筛选结果长度”。示例：

- `Client IP -> 127.0.0.1 (128)`
- `Applications -> WeChat (42)`
- `Domains -> api.example.com (315)`

用户可以据此判断“主要活跃来源”，然后再点击真正筛选。

### 为什么不做“动态联动计数”

理想化的 faceted search 是：

- 选中 `Client IP=A` 后，`Applications` 里每项显示“在 A 条件下还剩多少”。
- `Client IP` 自己维度会显示“排除本维度自身外的可叠加计数”。

第一版不做这一层，原因：

- 每次 filter 变化都要重新遍历所有 `records` 并重建三张聚合，成本 `O(n × m)`。
- 流量持续推送 + filter 频繁切换 = 主线程与虚拟表争抢渲染。
- 现有产品价值上，用户主要靠 count 判断“哪个来源多”，而不是“组合后还剩多少”。

因此第一版给用户提供“绝对分布”，不给“条件后分布”。

### 展示规范

- 计数徽标为弱化样式（灰色数字或 muted badge），不抢占选中态主色。
- `count = 0` 时不显示或显示 `0` 由组件层决定；实现上 `count ?? 0` 兜底。
- 面板搜索关键字变化只影响候选可见性，不影响 count 本身。

## 技术细节

### Store 层：三张增量计数 Map

`web/src/stores/useTrafficStore.ts`：

```ts
type TrafficState = {
  records: TrafficSummary[];
  clientIpCounts: Map<string, number>;
  clientAppCounts: Map<string, number>;
  domainCounts: Map<string, number>;
  // ...
};
```

维护路径固定为：

- `fetchInitialData()` / `backfillHistory()` / `fetchUpdates()` 拉到批量 record 时，逐条 `incrementCount`。
- `handleTrafficPush()` / `handleTrafficDelta()` 新增或替换 record 时增量维护。
- `handleTrafficDeleted()` 删除时逐条 decrement，减到 0 从 Map 里剔除。
- `clearTraffic()` 直接把三张 Map `clear()`。

`incrementCount(map, key)`：`map.set(key, (map.get(key) ?? 0) + 1)`；`null / undefined` 记入统一 fallback（例如空串）不进 Map。

### 组件层：Traffic 页面透传

`web/src/pages/Traffic/index.tsx`：

- 从 store 拿到 `clientIpCounts / clientAppCounts / domainCounts` 并作为 props 传给 `FilterPanel`：

```tsx
<FilterPanel
  clientIpCounts={ipCounts}
  clientAppCounts={appCounts}
  domainCounts={domainCounts}
  // ...
/>
```

### 组件层：FilterPanel 渲染 count

`web/src/components/FilterPanel/index.tsx`：

- 分别为 Client IPs / Applications / Domains 三段循环渲染 `FilterItem`。
- 每个 `FilterItem` 传入 `count={clientIpCounts.get(ip) ?? 0}` / `clientAppCounts.get(app) ?? 0` / `domainCounts.get(domain) ?? 0`。
- 组的头部标题 `count` 使用“候选个数”，正文行 count 使用“该值出现次数”，语义不同不混用。

`web/src/components/FilterPanel/PinnedFilters.tsx`：

- Pinned 项按 `filter.type` 查对应 map：

```tsx
count={
  filter.type === "client_ip"
    ? (clientIpCounts.get(filter.value) ?? 0)
    : filter.type === "client_app"
      ? (clientAppCounts.get(filter.value) ?? 0)
      : (domainCounts.get(filter.value) ?? 0)
}
```

- Pinned 组件带 `data-testid="pinned-filter-count-<type>"` 便于 UI E2E 断言。

### 性能约束

- 不新增后端 API，不新增 push 字段。
- 不在 render / `useMemo` 里对全量 `records` 重扫。
- 不因搜索关键字变化触发全量重建。
- 只增加 `Map.get(value)` 与少量 badge 渲染开销，相对现有虚拟滚动可忽略。

### 后续可扩展的第二阶段（不属于 V1）

如后续确认“基于其他 filter 的动态计数”值得做，建议：

1. 在 store 或独立 selector 增加 `facetCounts` 计算入口，复用编译后的 filter 条件。
2. 采用“排除自身维度外的其它条件”口径：算 `Applications` counts 时不应用 `selectedClientApps`。
3. 只在 `recordsMutation.version` / filter 条件变化时重算，用 `startTransition` 或 Worker 隔离主线程。
4. 允许用户配置“只显示 top N”降低视觉噪声。

在明确产品需求前不实现。

## CLI + Web + Admin API

### Web UI

- Traffic 页面左侧 Filter Panel + Pinned Filters 展示计数徽标。
- 无 URL 参数，无键盘快捷键。
- 折叠面板时不显示 count；展开时随组渲染。

### CLI

本能力属纯前端体验，不涉及 CLI 命令。

### Admin API

不新增 API，不改动响应结构。计数完全在前端 store 内维护。

## Sync 边界

- 计数只在浏览器 store 内维护，不参与设备同步。
- 多标签页各自维护自己的计数视图；不通过 sync 通道共享。
- Push 通道通过既有 traffic delta 分发，Push 客户端只负责传递 traffic 变更，不感知 count。

## Phase 1

- Store 里补齐三张 count Map 并接入所有 mutation 路径（`fetchInitialData` / `backfillHistory` / `fetchUpdates` / `handleTrafficPush` / `handleTrafficDelta` / `handleTrafficDeleted` / `clearTraffic`）。
- Traffic 页面透传三张 map 到 `FilterPanel`。
- `FilterPanel` / `PinnedFilters` / `FilterItem` 渲染 count 徽标并加 test-id。

## Phase 2

- 补齐 UI E2E：Filter Panel 计数、Pinned 计数、删除后 count 更新、清空后 count 消失。
- 更新 `human_tests/webui-traffic.md` 加入 Filter Panel 计数验收步骤。

## Phase 3

- 观察真实使用中是否有“动态联动计数”需求，若有则设计 `facetCounts` selector。
- 提供“候选 Top N”视觉降噪开关。

## Phase 4

- 文档更新：设计文档、human_tests、release note 中说明 count 口径。

## 测试方案

### 组件级 / 单元测试

- `web/src/components/FilterPanel/FilterItem.test.tsx`：给定 count 断言 badge 内容。
- `web/src/stores/useTrafficStore.test.ts` 相关用例：
  - `incrementCounts_from_fetchInitialData` — 首屏批量记录后三张 map 与 records 一致。
  - `incrementCounts_from_push_delta` — 单条 push 后对应 key +1。
  - `decrementCounts_from_deleted` — deleted 后 count -1，归零剔除 key。
  - `clearCounts_from_clearTraffic` — clearTraffic 后三张 map 为空。

### UI E2E（Playwright）

`web/tests/ui/traffic.spec.ts`（追加）：

- `filter panel shows counts per candidate` — 构造多组 client_ip / app / host，断言每行右侧展示对应数字。
- `filter panel counts update on delete` — 删除单条后目标行 count -1；删完某 key 后行消失。
- `pinned filter counts match panel counts` — Pin 一项后 `data-testid=pinned-filter-count-*` 与列表候选行 count 完全一致。
- `filter panel counts reset on clearTraffic` — 触发清空，所有候选行连同 count 一起消失。

### 人工验证 human_tests

在 `human_tests/webui-traffic.md` 追加：

- TC-FPC-01：三段候选每项展示 count，与 CLI `bifrost traffic list --format json | jq 'group_by(.host)'` 分布一致。
- TC-FPC-02：删除某条流量，对应 host 的 count -1。
- TC-FPC-03：Pinned 项与候选行 count 保持一致。

## Review / Fix / Test 闭环

### 第 1 轮

- 复核用户目标：候选 count 展示、Pinned 展示、随 mutation 同步、面板搜索不影响 count。
- 复核 diff：`useTrafficStore` mutation 路径是否全部接入 count；组件层是否直接读取 store map；PinnedFilters 是否复用同一套逻辑。
- 复测：单元测试 + Playwright + human_tests 首轮。

### 第 2 轮

- 复核第 1 轮修复；重点审查是否有隐藏的 mutation 路径漏调 count 维护（例如 batch 替换、状态更新）。
- 复测：删除、清空、初次进入的 count 时序；window hidden 恢复后 count 是否与 records 保持一致。

## 风险与决策

- **决策**：第一版只做“绝对分布”，不做“基于其他 filter 的动态分布”，避免主线程压力。
- **风险**：如果 mutation 路径新增而未同步 count 维护，会导致 count 与 records 漂移。缓解：所有 mutation 走同一组 helper（`incrementCount` / `decrementCount`），并在单元测试中覆盖每条 mutation 路径。
- **风险**：面板搜索关键字变化不能重算 count；实现里必须小心 memoization，避免误依赖搜索关键字。
- **决策**：Pinned 与普通候选共用同一 count 口径，不引入独立聚合，避免体验不一致。
- **决策**：不新增 Admin API 也不新增 push 字段，全部复用现有 store，最低成本上线。

## 文档更新要求

- 本次为 UI 体验增强，不涉及 README / Admin API / 配置项变更。
- 更新 `human_tests/webui-traffic.md` 与 `human_tests/readme.md`，加入 Filter Panel count 用例。
- 若后续引入“动态联动计数”，需再次更新本文档，避免设计与实现漂移。

## 依赖文件

- `web/src/stores/useTrafficStore.ts`
- `web/src/pages/Traffic/index.tsx`
- `web/src/components/FilterPanel/index.tsx`
- `web/src/components/FilterPanel/FilterItem.tsx`
- `web/src/components/FilterPanel/PinnedFilters.tsx`
- `web/tests/ui/traffic.spec.ts`
- `human_tests/webui-traffic.md`
