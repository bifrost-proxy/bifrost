# Web Admin 通知页顶部状态筛选设计

## 背景

Bifrost Web Admin `/notifications` 页面承载三种通知：

- `All Notifications`
- `TLS Trust`
- `Authorization`

旧版本三张表没有顶部筛选：用户进入通知页看到的是全部通知，未读消息容易被读过的消息稀释；同时表格默认允许用户切换 pageSize，出现过大 pageSize 造成一次性渲染大量记录的问题。

后端 `notifications.db` 早期没有清理策略：任何一次 push 都会新增 record，长时间运行后 `notifications.db` 持续膨胀。

本次优化在保持后端通知 API 签名不变的前提下：

- 三张表顶部提供 `All / Read / Unread` 状态筛选；首次进入默认选中 `Unread`。
- 分页固定 `pageSize = 10`，不允许 UI 切换。
- 通知持久化数据库执行清理策略：只保留最新 200 条，且淘汰 90 天以上的记录，每次 `create_notification` 写入后触发。

## 用户目标验证清单

### 必须实现

- 三张通知表顶部均显示 `All / Read / Unread` 状态筛选控件（Ant Design `Segmented`）。
- 首次进入某张表时默认选中 `Unread`。
- 切换筛选后立即刷新对应表数据。
- 表格分页显式 `showSizeChanger: false`，`pageSize: 10`。
- `handleMarkAllRead`、`handleUpdateStatus` 操作后按当前 Tab + 当前状态筛选刷新，不回退到未筛选列表。
- 后端 `notification_db` 每次写入后先淘汰 90 天以上记录，再按 `id DESC` 保留最新 200 条。

### 必须不破坏

- 通知 HTTP API：`GET /api/notifications`、`markAllRead`、`updateNotificationStatus` 保持既有签名，仅通过 `status` 查询参数区分。
- push 通道对通知的广播语义不变。
- 通知详情、跳转、passthrough 行为不变。
- `handleUpdateStatus` 支持 `read` / `dismissed`、`passthrough` / `ignored` 语义组合。
- 其它 admin push 订阅字段（`need_traffic` 等）不受影响。

### 必须真实验证

- Playwright 覆盖三张表首次默认 Unread、切换到 All / Read、分页无 pageSize 切换器。
- Rust 单测覆盖 `notification_db` 清理策略：写入超过 200 条后剩余数量正好等于 200；每次写入都触发清理。
- human_tests 覆盖真实浏览器手感与写入 205 条后清理结果。

## 产品语义

### 三张表共享同一份筛选组件

- 组件：`NotificationsTable`
- Props：`tabKey: 'all' | 'tls_trust_change' | 'pending_authorization'`
- 本地 state：`statusFilter: 'all' | 'read' | 'unread'`，默认 `unread`
- 数据流：`useEffect` 依赖 `activeTab === tabKey` 与 `statusFilter`，命中时调用 `fetchNotifications(tabKey, statusFilter)`。
- 切换 Tab 时由父 `NotificationsPage` 更新 `activeTab`，但每个 Tab 内的 `statusFilter` 独立维护。

### 状态筛选与后端参数映射

- `all` → 请求参数不带 `status`。
- `read` → `status=read`。
- `unread` → `status=unread`。

### 分页固定 10 条

- 表格 `pagination = { pageSize: 10, showSizeChanger: false }`。
- 服务端一次仅返回当前页记录；`pageSize` 由前端硬编码。

### 后端清理策略

- 常量：`MAX_NOTIFICATION_RECORDS: i64 = 200`；`MAX_NOTIFICATION_AGE_DAYS: i64 = 90`。
- `create_notification` 写入后立即调用 `cleanup_old_records(&conn)`。
- `do_cleanup(conn)`：
  1. `DELETE FROM notifications WHERE created_at < now - 90 days`
  2. 若剩余 count > 200，`DELETE FROM notifications WHERE id NOT IN (SELECT id ... ORDER BY id DESC LIMIT 200)`
- 每次写入触发保证长期运行 `notifications.db` 不无限膨胀。

## 技术细节

### 前端关键源码

- `web/src/pages/Notifications/index.tsx`
  - `type NotificationFilterStatus = 'all' | 'read' | 'unread';`
  - `NotificationsTable({ tabKey, statusFilter, ... })`
  - `Segmented` 控件生成 All / Read / Unread。
  - `useEffect` 依赖 `activeTab, fetchNotifications, statusFilter, tabKey`。
  - `pagination={{ pageSize: 10, showSizeChanger: false }}`。
- `web/src/stores/useNotificationStore.ts`
  - `fetchNotifications(type?: string, status?: string)`：`status === 'all' ? undefined : status`。
  - `handleMarkAllRead(type?: string, status?: string)`：写完后 `fetchNotifications(type ?? activeTab, status)`。
  - `handleUpdateStatus(id, status, action, type?, filterStatus?)`：写完后按 filter 刷新。
  - `setActiveTab(tab)`：仅设置 activeTab，实际请求由表组件根据自己的默认 filter 发起。
- `web/src/stores/useNotificationStore.test.ts`
  - 覆盖参数拼装与刷新行为。
- 测试 id：
  - `notifications-status-filter-all`
  - `notifications-status-filter-read`
  - `notifications-status-filter-unread`
  - `notifications-status-filter-tls_trust_change`（Tab 特有筛选定位）

### 后端关键源码

- `crates/bifrost-admin/src/notification_db.rs`
  - `const MAX_NOTIFICATION_RECORDS: i64 = 200;`
  - `const MAX_NOTIFICATION_AGE_DAYS: i64 = 90;`
  - `cleanup_old_records(&conn)` 包装 `do_cleanup(conn)`
  - `do_cleanup(conn)`：删过期 → 若超上限则按 `id DESC` 保留 200
  - 每次 `create_notification` 后调用 `cleanup_old_records`
- `crates/bifrost-admin/src/handlers/notification.rs`
  - HTTP handlers 使用 `status` 查询参数过滤；未变签名。
- `crates/bifrost-admin/src/push.rs`
  - `notification` push 广播不变。

### CLI + Web + Admin API

- CLI：无相关命令改动。
- Web：`Notifications/index.tsx`、`useNotificationStore.ts` 与相关 store 测试。
- Admin API：`GET /api/notifications` 支持 `status` 查询参数；`PUT /api/notifications/mark-all-read`、`PUT /api/notifications/{id}/status` 保持既有签名。

## Sync 边界

- 通知库为纯本地 SQLite 数据库，不参与 rule/value sync。
- 清理策略在本机进程内执行；多设备之间不同步。
- push 通知广播语义不变，不影响其它订阅方。

## 实现切分

### Phase 1：前端组件抽取

- `NotificationsPage` 拆分 `NotificationsTable`，接收 `tabKey`。
- 引入 `statusFilter` 本地 state。
- 顶部 `Segmented` 控件与切换事件。

### Phase 2：store 支持

- `fetchNotifications` 支持 `status`。
- `handleMarkAllRead` / `handleUpdateStatus` 支持透传 `type` + `status`。
- 更新 `useNotificationStore.test.ts` 覆盖新签名。

### Phase 3：后端清理

- 常量与 `do_cleanup(conn)`。
- `create_notification` 写入后触发。
- 单元测试覆盖 keep-below-limit、over-limit-trim、每次写入触发。

### Phase 4：分页固定 & 测试

- `pagination.showSizeChanger = false`；`pageSize = 10`。
- 新增 Playwright `web/tests/ui/notifications.spec.ts`。
- 新增 human_tests `human_tests/webui-notifications.md`。
- 同步 `human_tests/readme.md`。

## 测试方案

### 单元测试

- 前端：`web/src/stores/useNotificationStore.test.ts`
  - `TC-NOTIF-U01`：`fetchNotifications('tls_trust_change', 'unread')` 请求参数拼装正确。
  - `TC-NOTIF-U02`：`handleMarkAllRead(activeTab, statusFilter)` 后按 filter 刷新。
  - `TC-NOTIF-U03`：`handleUpdateStatus` 使用当前 Tab 与 filter 刷新。
- 后端：`crates/bifrost-admin/src/notification_db.rs` 内嵌 tests：
  - `TC-NOTIF-U04`：`test_cleanup_old_notifications` 删除 90 天以上记录。
  - `TC-NOTIF-U05`：`test_cleanup_keeps_records_when_below_limit` 少于 200 时不清理。
  - `TC-NOTIF-U06`：`test_cleanup_trims_to_latest_200_records` 写入 205 后剩 200。
  - `TC-NOTIF-U07`：`test_cleanup_runs_after_every_notification_write` 每次写入触发。
- 运行命令：
  - `pnpm --dir web exec vitest run src/stores/useNotificationStore`
  - `cargo test -p bifrost-admin notification_db --lib`

### E2E 测试

- Playwright：`web/tests/ui/notifications.spec.ts`
  - `TC-NOTIF-E01`：`Notifications tables default to unread filter and keep pagination size fixed`
    - 三张表顶部均显示筛选控件，默认展示 Unread 数据。
    - 切换到 All 后能看到 Read 数据。
    - 切换到 Read 后不再看到 Unread。
    - `.ant-pagination-options` 不出现，验证 pageSize 切换器已隐藏。
    - Tab 切换后再切筛选控件仍然正常。
- 相关后端 e2e（用于验证 push + 清理不破坏其他功能）：`e2e-tests/tests/` 下 notification 相关脚本。

### 真实场景测试 human_tests

- `human_tests/webui-notifications.md`
  - `TC-NOTIF-H01`：三张通知表默认展示 Unread；切换 All / Read / Unread 正确。
  - `TC-NOTIF-H02`：分页固定 10 条，不显示 pageSize 切换器。
  - `TC-NOTIF-H03`：真实生成 205 条通知后 SQLite 中只剩最新 200 条。
  - `TC-NOTIF-H04`：`Mark all read` 后按当前 filter 刷新，不回退到 All。
- 同步更新 `human_tests/readme.md` 中通知分组用例数与说明。

### 覆盖率与项目校验

- `pnpm --dir web exec tsc --noEmit`
- `pnpm --dir web exec vitest run src/stores/useNotificationStore`
- `pnpm --dir web exec playwright test tests/ui/notifications.spec.ts`
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test -p bifrost-admin notification_db --lib`
- `cargo test --workspace --all-features`（若资源允许）
- `bash scripts/ci/local-ci.sh --skip-e2e`
- 按 `rust-project-validate` 执行；如无法完整跑 workspace 测试，需在 PR 中记录阻塞与替代验证。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核用户目标：三张表默认 Unread、切换刷新、分页固定 10、后端清理策略。
- 重点 review：
  - `NotificationsTable` 是否正确响应 `tabKey === activeTab` 才发起请求，避免非激活 Tab 的多余请求。
  - `handleMarkAllRead` / `handleUpdateStatus` 是否忘记传入当前 Tab 或 filter。
  - `do_cleanup` 是否严格先删过期再按 id DESC 裁剪，避免误删。
  - HTTP handler 的 `status` 参数解析未破坏旧调用者。
- 复跑受影响 vitest / Playwright / cargo test。

### 第 2 轮

- 复审第 1 轮修复后的最新 diff。
- 重点 review：
  - push 通道触发的实时通知在 Unread filter 下能立刻上屏。
  - `Mark all read` 在筛选状态下的按钮语义是否符合预期（仅当前 Tab 或全局？以现有实现为准，避免行为误伤）。
  - 后端清理是否会与并发写入产生锁竞争。
- 复跑受影响测试；若仍有回归追加第 3 轮直到关闭。

## 风险与决策点

- **默认 Unread 是否合适**：产品视角认为“进入通知页首要看待办”，第一版默认 Unread；若数据表明多数用户想看历史，可以改成默认 All。
- **200 条上限**：本机常规使用足够；若某个环境频繁触发通知，可能造成刚推送的通知很快被淘汰。可以将 `MAX_NOTIFICATION_RECORDS` 提到 config 项。
- **每次写入触发清理**：SQLite 单机可承受；若未来通知写入频率很高，可以改为节流（例如每 N 次写入触发一次或用后台 tick）。
- **pageSize 固定**：牺牲了个别高级用户想一次看更多的诉求；如产品要求恢复，可以放开 pageSize 且服务端做 hard cap（例如 100）。
- **筛选控件与 URL**：目前 `statusFilter` 只在组件内存；未来若要支持通过 URL 分享带筛选的通知视图，需要把它同步进 route search params。
