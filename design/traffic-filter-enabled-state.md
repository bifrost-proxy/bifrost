# Traffic Filter Enabled State

## 功能模块详细描述

Traffic 页主筛选器支持临时停用单条筛选条件。用户可以通过条件行前的 checkbox 控制该条件是否参与过滤，无需删除并重新创建条件。新增条件默认启用，避免用户添加后还需要额外操作。

## 实现逻辑

- `FilterCondition` 增加 `enabled?: boolean` 字段。
- 前端过滤逻辑统一按 `enabled !== false` 判断条件是否生效，兼容历史 URL、持久化状态和旧数据。
- `FilterBar` 在每个筛选条件行首渲染 checkbox：
  - checkbox 默认选中。
  - 取消勾选后，该条件保留在 UI 中但不参与 Traffic 列表过滤。
  - 删除按钮仍可使用，方便用户彻底移除条件。
- Traffic URL 反序列化时，历史条件缺少 `enabled` 字段会补为 `true`。
- Fuzzy Search 构造请求时只发送启用且有值的条件，停用条件不会误伤搜索结果。

## 依赖项

- `web/src/types/index.ts`
- `web/src/components/FilterBar/index.tsx`
- `web/src/stores/useTrafficStore.ts`
- `web/src/pages/Traffic/index.tsx`
- `web/src/components/SearchMode/index.tsx`
- `web/tests/ui/traffic.spec.ts`
- `human_tests/webui-traffic.md`
- `human_tests/readme.md`

## 测试方案

- 单元测试：`filterRecords ignores disabled filter conditions` 验证停用条件不参与本地列表过滤。
- 单元测试：`filterRecords treats legacy conditions without enabled as active` 验证旧条件默认仍生效。
- E2E 测试：`主筛选器支持临时停用单条条件` 覆盖：
  - 新增条件 checkbox 默认选中。
  - 输入 Path 条件后列表只保留匹配流量。
  - 取消勾选后不删除条件，列表恢复显示其他流量。
  - 重新勾选后条件再次生效。
- 真实场景测试：在 `human_tests/webui-traffic.md` 新增 Traffic 主筛选器临时停用用例，并按文档逐条执行。

## 校验要求

- 执行 `pnpm --dir web test:unit -- useTrafficStore.test.ts`。
- 执行 `pnpm --dir web test:ui traffic.spec.ts -g "主筛选器支持临时停用单条条件"`。
- 执行 `pnpm --dir web exec tsc -b --pretty false`。
- 执行 `cargo fmt --all -- --check`。
- 执行 `cargo clippy --workspace --all-targets --all-features -- -D warnings`。
- 执行 `cargo test --workspace --all-features`。
- 按修改范围评估并执行 `scripts/ci/local-ci.sh --skip-e2e`。

## 文档更新要求

- 本次是 WebUI Traffic 交互能力补充，不涉及 README、CLI 或公开 API 变更。
- 必须同步更新 `human_tests/webui-traffic.md` 与 `human_tests/readme.md`。
