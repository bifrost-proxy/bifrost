# Replay 首次保存后的历史绑定

## 功能模块详细描述

修复 Replay 页面从未保存请求（尤其是从 Traffic 导入后的请求）首次“保存为模板”后，执行请求看不到历史记录的问题。

当前表现是：

- 从 Traffic 导入到 Replay 后，请求默认处于 `unbound` 历史范围
- 首次保存为模板后，页面没有把当前请求切换成“已保存请求上下文”
- 随后执行请求时，历史虽然写入了绑定 `request_id` 的记录，但 History 页面仍按 `unbound` 过滤，导致列表为空
- 刷新页面后，因为未持久化当前选中的 saved request，用户也无法自动回到这条模板的历史上下文

## 实现逻辑

1. 在 `saveRequest` 成功后，无论是新建保存还是更新已保存请求，都统一回写最新 `currentRequest`。
2. 首次保存成功后立即切换 Replay 历史过滤条件为：
   - `historyFilter = { type: "request", requestId: savedRequest.id }`
3. 同步更新 UI 状态：
   - `selectedRequestId = savedRequest.id`
   - 清空 `selectedHistoryId`
   - 重置 `historyPage = 1`
4. 立即重新拉取该模板的最近历史；如果当前已经在 History 模式，再同步刷新分页历史。
5. 增加 UI 回归用例，覆盖“Traffic 导入 -> 保存模板 -> 执行 -> 查看历史 -> 刷新后再次查看”的完整链路。

## 依赖项

- `web/src/stores/useReplayStore.ts`
- `web/tests/ui/admin-replay.spec.ts`
- `web/src/pages/Replay/index.tsx`

## 测试方案（含 e2e）

1. 生成一条 Traffic 记录，并通过右键菜单导入到 Replay。
2. 在 Replay 中首次保存为模板。
3. 执行该模板请求并切换到 History。
4. 断言 History scope 显示当前模板名，且执行记录可见。
5. 刷新页面后再次切到 History，断言模板自动恢复选中且历史仍可见。
6. 按任务要求先执行 Replay UI E2E，再执行 rust-project-validate。

## 校验要求（含 rust-project-validate）

- 先执行本次变更相关的 Replay UI E2E 回归
- 再依次执行：
  - `cargo fmt --all -- --check`
  - `cargo clippy --all-targets --all-features -- -D warnings`
  - `cargo test`（按修改范围执行）
  - `cargo build --all-targets --all-features`

## 文档更新要求

- 本次为管理端交互修复，不涉及 README、API 或配置项变更

## 实现现状（截至 2026-06-17）

- `web/src/stores/useReplayStore.ts` 的 `saveRequest` 已在保存成功后回写 `currentRequest`，将 `historyFilter` 切换为 `{ type: request, requestId: savedRequest.id }`，并同步重置 `selectedRequestId`、`selectedHistoryId`、`historyPage`；随后立即调用 `loadRecentHistory`，且在 `uiState.mode === history` 时再触发 `loadAllHistory`。
- 持久化层 `partialize` 已包含 `selectedRequestId`，刷新后 `web/src/pages/Replay/index.tsx` 内的 effect 会依据持久化的 `selectedRequestId` 在 `savedRequests` 中找到记录并调用 `selectRequest`，从而恢复模板上下文。
- e2e 用例 `web/tests/ui/admin-replay.spec.ts` 中的 `从 Traffic 导入后首次保存的模板在执行后可见历史，刷新后仍能恢复` 覆盖了 Traffic 导入 → 首次保存 → 执行 → History 查看 → 刷新后再次查看 的完整链路。
- 上述能力均已落地；本设计文档无 "(planned, not yet shipped as of 2026-06-17)" 项。
