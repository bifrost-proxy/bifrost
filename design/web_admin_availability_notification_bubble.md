# Web Admin 可用性检查通知气泡交互设计

## 背景

管理端 Layout 全局挂载了一个浮动 `AvailabilityCheckNotificationCenter` 气泡，用于提示移动端可用性检查页面（mobile availability terminal / trust probe）当前有活跃探测设备接入。原先气泡直接 pin 在页面右上角、无位移能力，实际使用中出现三类问题：

1. 气泡默认位置贴在导航栏之上，Toolbar 上的搜索、状态提示常被遮挡。
2. 通知只是静态 badge，用户容易忽略，尤其在暗色主题下。
3. 用户无法把气泡移到不影响自己工作的位置，导致长时间开着可用性检查页面时体验很差。

本次优化在保持后端 `trust_probe` push 语义完全不变的前提下，重构折叠态气泡的交互：

- 默认位置从贴顶下移 72px，避开 Toolbar。
- 折叠态支持鼠标 + 触控 pointer drag；用户可拖动到视口任意位置。
- 拖动距离超过阈值时不触发展开点击。
- 存在待处理通知时增加轻微 bounce 动画与外圈 pulse 动画。
- 使用 `prefers-reduced-motion: reduce` 关闭循环动画，符合系统级动画减少偏好。
- 位置裁剪逻辑抽出到 `position.ts` 纯函数，方便单元测试。

实现文件集中在 `web/src/components/AvailabilityCheckNotificationCenter/`。

## 用户目标验证清单

### 必须实现

- 首次进入管理端页面时，通知气泡默认位置为 `right: 18 + 42, top: 72`（见 `NOTIFICATION_BUBBLE_MARGIN=18`、`NOTIFICATION_BUBBLE_SIZE=42`、`NOTIFICATION_BUBBLE_DEFAULT_TOP=72`）。
- 折叠态气泡可以被鼠标或触控指针拖动。
- 拖动过程中容器切换为 `left/top` 绝对定位，并通过 `clampNotificationBubblePosition()` 限制在 `[margin, viewport - size - margin]` 内，不会拖出视口。
- 拖动距离超过阈值时释放不触发点击展开；小于阈值的按下-释放依然打开卡片。
- 存在通知时气泡出现 bounce + pulse 动画；无通知时保持静止。
- `prefers-reduced-motion: reduce` 时动画完全关闭。
- 展开面板复用当前浮动位置作为锚点，通过 `clampNotificationPanelPosition()` 保证面板不会溢出视口。

### 必须不破坏

- `pushService` 对 `trust_probe` push 消息的订阅、去重、dismiss 语义不变。
- 现有 `TrustProbeSession` 数据模型不变。
- Ant Design token（主题色、阴影、Badge）不硬编码；亮暗主题一致。
- 展开卡片的现有列表、按钮、`Not now` 逻辑不变。
- 移动端可用性检查后端接口 (`crates/bifrost-admin/src/handlers/trust_probe.rs`、`crates/bifrost-admin/src/mobile_availability.rs`) 不需要改动。

### 必须真实验证

- 单元测试覆盖默认位置、视口边界裁剪、面板锚点裁剪。
- Playwright 覆盖：默认位置低于 Toolbar、拖拽后位置变化、拖拽后点击仍能展开、动画 class 存在。
- human_tests 覆盖：真实浏览器手感、拖拽误触点击验证、`prefers-reduced-motion` 场景。

## 产品语义

### 折叠态是持久提醒入口，不是完全静默

`AvailabilityCheckNotificationCenter` 是一个 always-mount 的浮动组件。只要 `pushService` 报告存在活跃 `trust_probe` session：

- Badge 显示未处理数量。
- 气泡带 bounce + pulse 动画。
- 用户可以拖到不挡工作区的位置，位置只影响当前会话，不持久化到 localStorage（第一版保守）。

### 拖拽与点击互斥

`AvailabilityCheckNotificationCenter/index.tsx` 通过 `dragStateRef` 记录：

- `pointerId`
- `startX / startY`
- `offsetX / offsetY`（指针相对气泡左上角偏移）
- `hasMoved`（是否越过阈值）
- `captured`（是否已 setPointerCapture）

拖拽过程：

- pointerdown → 记录起点，不立刻 capture。
- pointermove → 若移动距离超过阈值，标记 `hasMoved = true` 并 `setPointerCapture`，同时切换到 `left/top` 定位模式。
- pointerup / pointercancel → 若 `hasMoved` 为 true，认定为拖拽结束，抑制原生 click。否则允许后续 click 展开。

### 视口裁剪由纯函数负责

`position.ts` 导出：

- `NOTIFICATION_BUBBLE_SIZE = 42`
- `NOTIFICATION_BUBBLE_MARGIN = 18`
- `NOTIFICATION_BUBBLE_DEFAULT_TOP = 72`
- `NOTIFICATION_PANEL_WIDTH = 360`
- `defaultNotificationBubblePosition(viewport)`：右上角默认位置。
- `clampNotificationBubblePosition(pos, viewport, bubbleSize?, margin?)`
- `clampNotificationPanelPosition(pos, viewport, panelWidth?, margin?)`

纯函数使得 `position.test.ts` 能覆盖：

- 视口远大于气泡时默认位置不受影响。
- 视口过小时裁剪回落到最小 margin。
- 拖到左/右/上/下越界时都能钳制回可见范围。
- 面板宽度不同于气泡时的独立钳制。

## 技术细节

### 关键源码

- `web/src/components/AvailabilityCheckNotificationCenter/index.tsx`
  - `dragStateRef` 记录拖拽状态
  - `useState<FloatingPosition | null>(null)` 保存 `draggedPosition`
  - `onPointerDown` / `onPointerMove` / `onPointerUp` / `onPointerCancel` 完整生命周期
  - `activePosition = draggedPosition ?? defaultNotificationBubblePosition(viewport)`
  - `panelPosition = clampNotificationPanelPosition(activePosition, viewport)`
- `web/src/components/AvailabilityCheckNotificationCenter/position.ts`
  - 常量与钳制纯函数
- `web/src/components/AvailabilityCheckNotificationCenter/position.test.ts`
  - 覆盖默认位置、边界裁剪、面板位置
- `web/src/components/AvailabilityCheckNotificationCenter/index.css`
  - `@keyframes availability-check-bubble-bounce`（2.6s ease-in-out infinite）
  - `@keyframes availability-check-bubble-pulse`（2.6s ease-out infinite）
  - `@media (prefers-reduced-motion: reduce) { animation: none; }`

### 数据流

1. `pushService` 广播 `trust_probe` 类型 push。
2. `AvailabilityCheckNotificationCenter` 用 `useState<TrustProbeSession[]>` 维护 sessions。
3. `sessions.length > 0` 时给气泡加动画 class；否则移除。
4. `expanded` 状态切换展开面板，面板复用 `AvailabilityCheckPanel` 展示。

### 后端 API 边界

- `crates/bifrost-admin/src/handlers/trust_probe.rs` push 广播不变。
- `crates/bifrost-admin/src/mobile_availability.rs` HTTP handler 不变。
- `crates/bifrost-admin/src/push.rs` 中的 push 事件枚举不变。

### CLI + Web + Admin API

- CLI：无改动。
- Web：`AvailabilityCheckNotificationCenter/{index.tsx, position.ts, position.test.ts, index.css}`。
- Admin API：无接口签名变化。

## Sync 边界

- 拖拽后的位置不同步到 sync。
- 不持久化到 localStorage（第一版保守）。
- 通知本身来自 push 通道；sync 语义不受影响。
- 后端 `trust_probe` push 通道不改动，不影响其他订阅方（如 mobile availability terminal）。

## 实现切分

### Phase 1：位置纯函数

- 抽出 `position.ts`，导出常量与 `clamp*` 函数。
- 编写 `position.test.ts` 覆盖默认与边界。

### Phase 2：拖拽交互

- `index.tsx` 引入 `dragStateRef`、`draggedPosition` 状态。
- 实现 `onPointerDown/Move/Up/Cancel`，正确处理 pointer capture。
- 拖拽阈值抑制 click。

### Phase 3：动画

- `index.css` 增加 bounce + pulse `@keyframes`。
- 气泡在 `sessions.length > 0` 时加动画 class。
- `@media (prefers-reduced-motion: reduce)` 关闭动画。

### Phase 4：测试与 human_tests

- Playwright：`web/tests/ui/notifications.spec.ts` 增加气泡拖拽与位置断言。
- human_tests：`human_tests/webui-notifications.md` 覆盖默认位置、拖拽、动画、`prefers-reduced-motion`。
- 同步 `human_tests/readme.md` 用例数。

## 测试方案

### 单元测试

- `web/src/components/AvailabilityCheckNotificationCenter/position.test.ts`
  - `TC-BUBBLE-U01`：默认位置在正常视口下贴右侧、下移 72px。
  - `TC-BUBBLE-U02`：拖到超越右下边界时被钳制回视口。
  - `TC-BUBBLE-U03`：拖到 `left < margin` 时钳制到 `margin`。
  - `TC-BUBBLE-U04`：面板位置独立钳制，`panelWidth=360`。
- 运行：`pnpm --dir web exec vitest run src/components/AvailabilityCheckNotificationCenter`

### E2E 测试

- `web/tests/ui/notifications.spec.ts`
  - `TC-BUBBLE-E01`：mock `trust_probe` push 后，气泡出现在默认位置且低于顶部 Toolbar。
  - `TC-BUBBLE-E02`：气泡带 bounce / pulse 动画 class。
  - `TC-BUBBLE-E03`：Playwright pointer drag 后，气泡 `left/top` 发生变化且仍在视口内。
  - `TC-BUBBLE-E04`：拖拽后释放，再单击气泡仍能展开面板。
  - `TC-BUBBLE-E05`：分页/pagesize（属于通知表其他用例）与气泡拖拽互不影响。
- 相关文件：`web/tests/ui/notifications.spec.ts`

### 真实场景测试 human_tests

- `human_tests/webui-notifications.md`
  - `TC-BUBBLE-H01`：默认位置低于 Toolbar，不遮挡搜索。
  - `TC-BUBBLE-H02`：鼠标拖拽气泡到左下角，释放后位置保留。
  - `TC-BUBBLE-H03`：拖拽气泡不触发展开；单击气泡展开面板。
  - `TC-BUBBLE-H04`：系统偏好设置为 reduce motion 时动画消失。
  - `TC-BUBBLE-H05`：暗色主题下气泡颜色、阴影正常，无硬编码颜色偏差。
- 同步更新 `human_tests/readme.md` 中 `webui-notifications` 分组用例数与说明。
- 关联真实场景：`human_tests/mobile-availability-terminal.md`、`human_tests/mobile-device-trust.md`。

### 覆盖率与项目校验

- `pnpm --dir web exec tsc --noEmit`
- `pnpm --dir web exec vitest run src/components/AvailabilityCheckNotificationCenter`
- `pnpm --dir web exec playwright test tests/ui/notifications.spec.ts`
- `rust-project-validate`：本次改动不涉及 Rust 代码，只需最小验证；如果全量 `cargo test --workspace --all-features` 因环境资源无法运行，需要在 PR 中记录并附替代验证证据。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核用户目标：默认位置、拖拽、动画、视口裁剪、`prefers-reduced-motion`。
- 复核 diff：`index.tsx`、`position.ts`、`position.test.ts`、`index.css` 是否都更新。
- 重点 review：
  - `dragStateRef` 是否 pointer capture 与 release 成对，避免拖出视口后卡住。
  - 拖拽阈值与点击互斥逻辑，防止误触。
  - `clampNotificationPanelPosition` 是否被正确复用到展开态。
  - 亮暗主题下动画阴影是否用 token。
- 运行：单元测试、Playwright 通知 spec、human_tests 气泡用例。

### 第 2 轮

- 复审第 1 轮修改后的最新 diff。
- 重点 review：
  - 小视口（宽 < 400px）下气泡是否仍可点击。
  - 触控设备（Playwright `--tap`）下 pointer 事件序列。
  - `prefers-reduced-motion` 切换是否被 CSS media query 实时响应。
- 复跑受影响测试；若仍有回归追加第 3 轮直到关闭。

## 风险与决策点

- **位置持久化**：第一版不写 localStorage，避免多标签页跨会话冲突。若产品需求要求，后续再决定 key 命名与失效策略。
- **拖拽阈值**：当前用固定像素阈值；触控设备可能需要更大阈值防误触，后续可从常量升级为按设备类型区分。
- **动画性能**：bounce + pulse 都是 CSS keyframes，无 JS 主循环，performance impact 极小；但 pulse 使用 `box-shadow`，若发现低端 Mac 上有 paint 抖动，可换成 `transform: scale`。
- **可用性检查通知量**：`trust_probe` push 通常量级很低（同一时刻个位数 session），当前实现不做批量合并；如果未来场景变化再考虑合并策略。
- **不覆盖 dismiss/keep 逻辑**：展开面板内 dismiss / keep 交互由既有 `AvailabilityCheckPanel` 承担，本次不改。
