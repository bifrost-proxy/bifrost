# Traffic Filter Panel Counts

## 功能模块详细描述

在 Network/Traffic 页面左侧 `Filters` 面板中，为每一项筛选值展示对应请求数量，例如：

- `Client IP -> 127.0.0.1 (128)`
- `Applications -> WeChat (42)`
- `Domains -> api.example.com (315)`

目标是让用户在不点击筛选项的情况下，快速判断哪一类来源更活跃，同时不引入新的高频全量扫描或额外后端查询。

## 现状分析

- `web/src/stores/useTrafficStore.ts` 已维护三组实时聚合：
  - `clientIpCounts: Map<string, number>`
  - `clientAppCounts: Map<string, number>`
  - `domainCounts: Map<string, number>`
- 这些计数不是渲染时临时 `records.filter(...).length` 算出来的，而是在以下路径中增量维护：
  - `fetchInitialData`
  - `backfillHistory`
  - `fetchUpdates`
  - `handleTrafficPush`
  - `handleTrafficDelta`
  - `handleTrafficDeleted`
  - `clearTraffic`
- 左侧 `FilterPanel` 当前只消费：
  - `availableClientIps`
  - `availableClientApps`
  - `availableDomains`
- 因此，最适合的方案不是增加新接口，也不是在组件层重复遍历 `records`，而是直接把 store 里已经维护好的计数透传到 UI。

## 实现逻辑

### 方案选择

采用“两层计数”中的第一层能力：

- 展示每个筛选值在当前 `records` 窗口中的基础计数
- 计数口径与左侧候选列表保持一致，即“当前已加载到前端内存中的 traffic summary 数量”
- 不随 toolbar filter / add filter / 其他 panel filter 的变化重新全量计算

这是当前性价比最高、对性能最友好的方案。

### 前端改动

1. 扩展 `Traffic` 页传给 `FilterPanel` 的 props：
   - `clientIpCounts`
   - `clientAppCounts`
   - `domainCounts`

2. 扩展 `FilterPanel`：
   - 在渲染 `FilterItem` 时传入对应 `count`
   - `PinnedFilters` 也复用同一套 count 展示，避免固定项和普通列表体验不一致

3. 扩展 `FilterItem` 展示：
   - 右侧增加一个弱化样式的数字 badge 或灰色 count 文本
   - 选中态只改变主色，不改变数字语义
   - 搜索过滤时仍展示原始 count，不单独为搜索关键字再算一次命中数

### 性能约束

这个方案不新增以下开销：

- 不新增后端 API
- 不新增 push 字段
- 不在 render / `useMemo` 中对 `records` 做 `O(n * m)` 级重扫
- 不因左侧搜索关键字变化而重建计数 map

新增开销只有：

- `Map.get(value)` 读取计数
- 少量 badge/text 渲染

这部分开销相对当前列表虚拟滚动、详情请求和增量同步可以忽略。

## 为什么不建议第一版做“动态联动计数”

更“理想化”的交互通常是：

- 选中 `Client IP=A` 后
- `Applications` 区域中的每一项显示“在 A 条件下还剩多少”
- 且 `Client IP` 自己这一组可能显示“排除本维度自身后的可叠加计数”

这类计数属于 faceted search / contextual counts。若直接在前端每次筛选变化后对所有 records 和所有 facet 重新计算，会带来明显 CPU 成本：

- 每次 filter 变化都要重新遍历 `records`
- 还要分别按 app / ip / domain 重建聚合
- 在流量持续推送、filter 频繁切换时，容易与主列表过滤竞争主线程

因此不建议首版直接上这类动态计数。

## 后续可扩展的第二阶段

如果后续确认需要“基于当前其他筛选条件的动态计数”，建议按下面约束实现，而不是直接在组件层现算：

1. 在 `useTrafficStore` 或独立 selector 中增加 `facetCounts` 计算入口
2. 复用已编译的 filter 条件，避免每次重复 `compileConditions`
3. 采用“排除当前 facet、自身维度外的其它条件”口径：
   - 计算 `Applications` counts 时，应用 toolbar + add filter + selectedClientIps + selectedDomains
   - 但不应用 `selectedClientApps`
4. 仅在以下时机重算：
   - `recordsMutation.version` 变化
   - toolbar/filter/panel 条件变化
5. 必要时放进 `startTransition` 或 Worker，避免与主表渲染抢占主线程

在没有明确产品要求前，不建议直接进入第二阶段。

## 依赖项

- `web/src/stores/useTrafficStore.ts`
- `web/src/pages/Traffic/index.tsx`
- `web/src/components/FilterPanel/index.tsx`
- `web/src/components/FilterPanel/FilterItem.tsx`
- `web/src/components/FilterPanel/PinnedFilters.tsx`

## 测试方案（含 e2e）

以下用例 (planned, not yet shipped as of 2026-06-17) — 当前仓库中 `web/` 下尚未提交 `FilterPanel` / `FilterItem` / `PinnedFilters` 的单元测试或对应 Traffic 计数的 e2e 用例，后续补齐时按以下口径展开：

- 组件级验证：
  - 构造包含多个 client ip / app / domain 的 records
  - 断言左侧每一项展示的数字与 store 中对应 count 一致
  - 删除记录、导入记录、push 更新后，断言 count 同步变化
- UI E2E：
  - 打开 Traffic 页面并制造多组不同来源请求
  - 断言左侧 Filters 每项出现数量
  - 删除某一组请求后，断言数量减少
  - 清空全部流量后，断言候选项与数量一起清空

## 校验要求（含 rust-project-validate）

- 先执行与本次改动相关的 Traffic UI E2E
- 再执行 `rust-project-validate` 要求的 fmt / clippy / test / build

## 文档更新要求

- 本次为 UI 体验增强，不涉及 README / API / 配置项变更
- 若最终交互与本文不同，应继续增量更新本文件，避免方案与实现漂移
