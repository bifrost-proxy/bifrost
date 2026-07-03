# Traffic 主筛选器条件级停用（Enabled State）

## 背景

Bifrost Web Admin 的 Traffic 页顶部有一个“主筛选器”（FilterBar），支持
用户按 Path / Host / Method / Status / Header / Request Body / Response
Body 等字段增删多条 `FilterCondition`。历史行为下，用户只能通过“新增
条件 + 删除条件”来短暂调试某一个筛选器的影响；每次都要重新填写字段、
运算符、值，操作成本很高。

新语义引入“条件级 enabled”：每条 `FilterCondition` 前多一个 checkbox，
默认勾选。取消勾选时条件保留在 UI 中，但不参与本地 filter 与后端 fuzzy
search 请求。用户可以频繁切换某条条件是否生效，而不丢失已填内容。

## 用户目标验证清单

### 必须实现

- `FilterCondition` 新增 `enabled?: boolean` 字段。
- 前端本地筛选统一按 `condition.enabled !== false` 判断是否生效，未指定
  时视为启用（兼容历史 URL、持久化状态、旧数据）。
- FilterBar 每条条件行首渲染 `Checkbox`：
  - 默认 `checked`。
  - `checked=false` 时该条件在 UI 保留但不参与过滤，字段 / 运算符 / 值
    输入禁用。
  - 删除按钮仍可用，方便彻底移除。
- Traffic URL 反序列化：历史条件缺 `enabled` 字段时补为 `true`。
- Fuzzy Search 请求构造：只把 `enabled` 且有值的条件放入 `conditions`
  数组，停用条件不发送到后端。
- 新增条件时 `enabled` 缺省为 `true`。

### 必须不破坏

- 已有条件的 field / operator / value / id 序列化行为不变。
- URL / localStorage 持久化的旧格式（无 `enabled`）能被读回并显示为“启用”。
- Toolbar 快速筛选（Rule / Protocol / Status / Type / Imported）与条件级
  enabled 无关，行为不变。
- Fuzzy search 后端不感知 `enabled`，仅拿到 enabled 条件集合。
- `is_empty` / `is_not_empty` 运算符即使 `value` 为空也算 applicable，
  这条特例不变。

### 必须真实验证

- 新增条件时 checkbox 默认勾选。
- 输入 Path 条件后列表只保留匹配行；取消勾选后列表恢复；再次勾选后
  重新生效。
- 停用条件 UI 中依然显示原字段 / 运算符 / 值。
- Fuzzy search 场景（切换到 Search Mode）停用条件不参与请求。

## 产品语义

### `enabled` 是可选字段，缺省视为启用

`FilterCondition.enabled?: boolean`。为了向后兼容，所有 filter 判断都用
`condition.enabled !== false`。不用 `condition.enabled === true`，因为那样
旧数据（`enabled` 为 `undefined`）会被判成停用。

### 停用条件不删除

停用只影响“是否参与筛选”，条件本身仍在数组中，仍在 UI 上显示，仍占位。
用户可以修改 field / operator / value（在勾选状态下）或直接删除。

停用状态下字段 / 运算符 / value 输入应该禁用（`disabled={filter.enabled ===
false}`），提示用户当前条件不生效。

### 判定条件是否 applicable 的唯一入口

`isFilterConditionApplicable(condition)` 是唯一负责“这条条件对当前列表
是否有实际影响”的判断函数，两处必须保持一致：

- 本地 `filterRecords(records, toolbar, conditions)` 中的 `conditions.some(isFilterConditionApplicable)`；
- Fuzzy search 请求构造中的 `conditions.filter(isFilterConditionApplicable)`。

`isFilterConditionApplicable` 定义：

```ts
condition.enabled !== false &&
  (condition.operator === 'is_empty' ||
   condition.operator === 'is_not_empty' ||
   condition.value.trim().length > 0)
```

即：启用 && (空值/非空运算符 || value 非空)。

### 持久化与 URL 兼容

Traffic 主筛选器状态会同步到 URL query `?f=<encoded>` 与 localStorage。
读回时对每条条件补 `enabled = enabled ?? true`（或在使用点用 `!== false`
判定），确保旧链接不被解释成“全部停用”。

## 技术细节

### 关键代码入口

- `web/src/types/index.ts` (line ~465)
  ```ts
  export interface FilterCondition {
    id: string;
    field: string;
    operator: string;
    value: string;
    enabled?: boolean;
  }
  ```

- `web/src/stores/useTrafficStore.ts`
  - `isFilterConditionApplicable(condition)` (line ~555)
  - `hasActiveFilters(toolbar, conditions)` (line ~566) 使用
    `conditions.some(isFilterConditionApplicable)`
  - `filterRecords(records, toolbar, conditions)` 内部把 `conditions
    .filter(isFilterConditionApplicable)` 编译成 `CompiledCondition[]`。

- `web/src/components/FilterBar/index.tsx`
  - checkbox 渲染 (line ~232)：
    ```tsx
    <Checkbox
      checked={filter.enabled !== false}
      onChange={(e) => handleChange(filter.id, "enabled", e.target.checked)}
      aria-label="Enable filter"
      data-testid="traffic-filter-enabled-checkbox"
    />
    ```
  - `disabled={filter.enabled === false}` 应用于字段 / 运算符 / value 输入。

- `web/src/pages/Traffic/index.tsx`
  - `handleFilterConditionsChange(conditions)` 接收 FilterBar 完整数组，
    保留 `enabled` 字段。

- `web/src/components/SearchMode/index.tsx`
  - Fuzzy search 构造 `SearchRequestBody` 时使用 `conditions.filter(isFilterConditionApplicable)`
    投影为 `SearchFilterCondition[]`，`enabled` 字段本身不传给后端。

- Fuzzy search 后端类型 (line ~746)：
  ```ts
  export interface SearchFilterCondition {
    field: string;
    operator: string;
    value: string;
    // 无 enabled 字段
  }
  ```

### `handleChange(id, 'enabled', boolean)` 行为

- `handleChange` 是 FilterBar 内已有的 dispatch helper，`enabled` 通过
  它更新，即刻回写 store → 触发 `filterRecords` 重算 → 列表实时刷新。
- 因为 `filterVersion` 在 store 中依赖 conditions 变化，无需手动 bump。

### URL / localStorage 反序列化补丁

- 反序列化后遍历 conditions，若缺 `enabled` 字段补 `true`；或在所有
  使用处一律 `enabled !== false`。当前实现采用后者（消费点判定），避免
  写回时对旧数据做兼容层。

## CLI + Web + Admin API

### CLI

- 无 CLI 变更。CLI 端的 traffic 查询过滤走后端参数，与 UI 条件级 enabled
  无关。

### Web

- Traffic 页 FilterBar 每条条件行首渲染 checkbox。
- 停用状态下条件行整体灰化（视觉上），仍可展开 field / operator select
  但输入 disabled。
- 键盘可访问：checkbox 有 `aria-label="Enable filter"`。
- data-testid：`traffic-filter-enabled-checkbox` 用于 Playwright 定位。

### Admin API

- 无接口变更。Fuzzy search 请求 body 不含 `enabled`；后端只看到启用
  条件。
- `POST /_bifrost/api/traffic/search` request body 中 `conditions[]` 每项
  仍是 `{field, operator, value}`。

## Sync 边界

- 无。Traffic 主筛选器状态属于本地 admin UI 状态，不跨设备同步。

## Phase 1-4

### Phase 1（历史，已完成）

- `FilterCondition.enabled?: boolean` 类型扩展。
- `isFilterConditionApplicable` 从 `value.trim().length > 0`（含运算符
  例外）升级到叠加 `enabled !== false` 检查。
- `hasActiveFilters` / `filterRecords` 使用统一入口。

### Phase 2（历史，已完成）

- FilterBar UI 增加 checkbox + disabled input state。
- `data-testid="traffic-filter-enabled-checkbox"` 定位标识。

### Phase 3（历史，已完成）

- SearchMode 请求构造使用 `isFilterConditionApplicable` 过滤。
- URL / localStorage 反序列化在消费点用 `!== false` 判定，兼容旧数据。

### Phase 4（当前维护）

- 保持“默认启用、消费点判定、单一 applicable helper”三条不变量。
- 若未来 FilterBar 支持 grouping / OR，`enabled` 语义仍在单条条件维度
  生效。

## 测试方案

### 单元测试

- `web/src/stores/useTrafficStore.test.ts`
  - `Traffic filter condition enabled state > ignores disabled filter
    conditions`（line ~47）：验证 `enabled: false` 的条件不参与本地
    filter，`isFilterConditionApplicable` 返回 `false`。
  - `Traffic filter condition enabled state > treats legacy conditions
    without enabled as active`（line ~63）：验证缺 `enabled` 的旧数据仍
    生效。
- 建议补充：
  - `isFilterConditionApplicable returns true for is_empty operator with
    empty value when enabled`。
  - `filterRecords SearchMode request omits disabled conditions`。

### E2E

- `web/tests/ui/traffic.spec.ts > 主筛选器支持临时停用单条条件`（line ~858）
  覆盖：
  1. 加入两条流量记录，一个 targetPath 一个 otherPath。
  2. 打开 Traffic 页，两行都可见。
  3. 点击 “Add Filter” 添加空条件。
  4. 断言 `Enable filter` checkbox 默认勾选。
  5. 选择 Path 字段、填入 targetPath，仅 targetPath 行可见。
  6. 取消勾选 checkbox → 两行都可见（条件停用）。
  7. 再勾选 → 仅 targetPath 行可见（条件重新生效）。

### human_tests

- `human_tests/webui-traffic.md`：包含 “主筛选器条件级停用” 用例，
  指令用户按上述步骤手动验证，并记录 checkbox `aria-label` 与 disabled
  input 视觉效果。
- 已记录的执行命令：
  `pnpm --dir web test:ui traffic.spec.ts -g "主筛选器支持临时停用单条条件"`。

### 静态检查

- `pnpm --dir web test:unit -- useTrafficStore.test.ts`
- `pnpm --dir web exec tsc -b --pretty false`

## Review / Fix / Test 闭环

### 第 1 轮

- 复核 `isFilterConditionApplicable` 是过滤/搜索/`hasActiveFilters` 的
  唯一入口；grep `enabled !== false` 或 `enabled === false` 检查是否
  存在私自判断分支。
- 复核 FilterBar UI：checkbox 与 disabled input 联动、Delete 按钮仍可用。
- 复核 URL / localStorage 反序列化：旧链接不因 `enabled` 缺失被解释为
  全部停用。
- 复核 Fuzzy Search 请求：停用条件不出现在 request body。

### 第 2 轮

- Playwright “主筛选器支持临时停用单条条件” 用例回归。
- 手工在 Traffic 页面切换 enabled 多次，观察列表是否即时刷新，无卡顿。
- 复核多条件混合场景：一条 enabled、一条 disabled，只按 enabled 条件
  过滤。

## 风险与决策

- **决策**：`enabled` 使用 `enabled !== false` 判定而非 `enabled === true`。
  原因：向后兼容旧数据（`undefined` 视为启用）。
- **决策**：停用条件的字段 / 运算符 / 值 disabled，避免用户在停用态修改
  条件后再启用出现困惑。
- **决策**：`enabled` 字段不传给 fuzzy search 后端。原因：后端 API 保持
  向后兼容；前端在请求构造时 filter，缩小后端类型演进面。
- **风险**：如果未来引入 filter grouping（AND/OR/NOT），`enabled` 需要
  与 group 结构一起演进。当前实现只支持 flat AND，无 grouping 问题。
- **风险**：`is_empty` / `is_not_empty` 运算符在停用态下仍不 applicable，
  这是正确行为但需要在文档中显式说明避免误解为 bug。
