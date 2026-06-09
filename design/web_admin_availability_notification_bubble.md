# 管理端可用性检查通知气泡交互

## 功能模块详细描述

- 管理端全局浮动的 `Availability Check` 通知气泡用于提示移动端可用性检查页面有活跃设备访问。
- 本次优化折叠态气泡的用户感知：
  - 默认位置从顶部导航区域下移，避免遮挡 Toolbar。
  - 折叠态气泡支持鼠标和触控拖拽，用户可把提示移动到更合适的位置。
  - 存在通知时气泡显示轻微跳动和外圈脉冲动画，提示用户这里有通知信息。

## 实现逻辑

- `web/src/components/AvailabilityCheckNotificationCenter/index.tsx`
  - 新增浮动位置状态，默认使用右上角下移后的坐标。
  - 折叠态监听 pointer drag，拖动过程中把容器切换到 `left/top` 坐标并限制在视口内。
  - 拖动距离超过阈值时不触发打开，避免拖拽结束被误判为点击。
  - 展开态仍使用现有卡片展示逻辑，并复用当前浮动位置作为卡片锚点。
  - 新增位置裁剪纯函数，便于单元测试覆盖视口边界。
- `web/src/components/AvailabilityCheckNotificationCenter/index.css`
  - 增加通知气泡 bounce 与 pulse 动画。
  - 通过 `prefers-reduced-motion: reduce` 禁用循环动画，避免影响系统减少动效用户。
  - 保持按钮颜色和阴影来自 Ant Design token，不引入硬编码主题色作为状态依赖。

## 依赖项

- 复用现有 `pushService` 的 `trust_probe` 设置推送数据。
- 复用 Ant Design `Badge`、`Button`、`Card` 与图标，不新增运行时依赖。

## 测试方案

- 单元测试：
  - 新增 `web/src/components/AvailabilityCheckNotificationCenter/position.test.ts`
  - 验证默认位置下移后仍在视口内。
  - 验证拖拽坐标会被限制在视口边界内。
- E2E 测试：
  - 更新 `web/tests/ui/notifications.spec.ts`
  - 通过 mock `trust_probe` push 消息验证通知气泡可见、默认位置低于顶部 Toolbar、带有动画 class。
  - 用 Playwright 拖拽气泡，断言位置随拖拽变化且仍在视口内。
  - 点击拖拽后的气泡，断言仍能展开通知卡片。
- 真实场景测试（human_tests）：
  - 更新 `human_tests/webui-notifications.md`
  - 增加通知气泡默认位置、拖拽与动画提示用例。
  - 同步更新 `human_tests/readme.md` 中测试用例数与说明。

## Review/Fix/Test 闭环方案

- 第 1 轮：
  - 复核用户目标和当前 diff，重点检查拖拽误触点击、视口边界、亮暗主题样式和现有展开/关闭行为。
  - 运行通知相关单元测试、Playwright 通知用例和 human_tests 气泡用例。
- 第 2 轮：
  - 基于第 1 轮修复后的最新 diff 复核测试覆盖缺口，确认 `prefers-reduced-motion`、小视口和拖拽后点击行为没有回归。
  - 复跑受影响测试并确认不需要追加轮次。

## 校验要求（含 rust-project-validate）

- 先执行 Web 单元测试与通知 UI E2E。
- 再执行 human_tests 中新增/更新的通知气泡真实场景用例。
- 最后按项目规则执行 rust-project-validate；如完整 workspace all-features 因环境资源失败，必须记录命令、失败证据和风险。

## 文档更新要求

- 本次改动仅涉及管理端浮动通知气泡交互，无需更新 `README.md`。
- 需要同步更新 `human_tests/webui-notifications.md` 与 `human_tests/readme.md`。
