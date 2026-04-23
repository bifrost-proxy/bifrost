# 管理端通知页顶部状态筛选

## 功能模块详细描述

- 在 `/notifications` 页的三个通知表中新增顶部状态筛选，仅覆盖：
  - `All Notifications`
  - `TLS Trust`
  - `Authorization`
- 每个表顶部提供 `All`、`Read`、`Unread` 三个筛选项。
- 每个表首次进入时默认选中 `Unread`，优先展示待处理消息。
- 通知页表格分页保持默认页大小，不允许用户切换 page size。

## 实现逻辑

- `web/src/pages/Notifications/index.tsx`
  - 让三个通知 Tab 复用 `NotificationsTable` 时显式传入 `tabKey`。
  - `NotificationsTable` 内部维护局部 `statusFilter` 状态，默认值为 `unread`。
  - 当当前 Tab 激活或筛选项变更时，调用 `fetchNotifications(tabKey, statusFilter)` 拉取对应数据。
  - 顶部新增 `Segmented` 筛选控件，切换 `All / Read / Unread` 时刷新当前表格。
  - 表格分页显式设置 `showSizeChanger: false`，固定使用默认 `pageSize: 20`。
- `web/src/stores/useNotificationStore.ts`
  - `fetchNotifications` 支持同时透传通知类型与状态筛选。
  - `handleMarkAllRead`、`handleUpdateStatus` 支持按当前 Tab 与当前状态筛选刷新，避免操作后回退成未筛选列表。
  - `setActiveTab` 仅切换当前 Tab，由表格组件自己按默认筛选发起请求。

## 依赖项

- 复用现有通知接口 `getNotifications` 的 `status` 查询参数，无需新增后端 API。
- 复用现有通知状态更新接口 `markAllRead`、`updateNotificationStatus`。

## 测试方案

- 单元测试：
  - 新增 `web/src/stores/useNotificationStore.test.ts`
  - 验证 `fetchNotifications` 会按 tab/status 正确拼装请求参数
  - 验证 `handleMarkAllRead`、`handleUpdateStatus` 操作后会按当前筛选刷新
- E2E 测试：
  - 新增 `web/tests/ui/notifications.spec.ts`
  - 验证三个通知表顶部均显示状态筛选，默认只展示未读消息
  - 验证切换到 `All`、`Read` 后展示结果符合预期
  - 验证分页不显示 page size 切换器
- 真实场景测试（human_tests）：
  - 新增 `human_tests/webui-notifications.md`
  - 覆盖三张通知表默认未读、状态切换、固定分页三个核心场景

## 校验要求（含 rust-project-validate）

- 先执行通知页相关单元测试与 UI E2E。
- 再执行 `cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`cargo test --workspace --all-features`。
- 按前端改动范围执行 `bash scripts/ci/local-ci.sh --skip-e2e`。
- 最后执行 `rust-project-validate` 要求的项目校验。

## 文档更新要求

- 本次改动仅涉及管理端通知页交互，无需更新 `README.md`。
- 需要同步更新 `human_tests/readme.md` 索引。
